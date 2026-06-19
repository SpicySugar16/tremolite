use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use tremolite_llm::{
    LLMProvider, Message, PromptBuilder, PromptContext, PromptContributor, ProviderRegistry,
    ToolCallLoop, ToolCallRecord, ToolExecutor,
};

pub mod gateway;
pub mod module;
// 模块实现（内部使用，外部通过独立 crate 访问）
mod modules;
// 模块类型重导出——内建模块对外暴露的接口
#[doc(hidden)]
pub use modules::emotion::EmotionModule;
#[doc(hidden)]
pub use modules::memory::MemoryModule;
#[doc(hidden)]
pub use modules::attention::AttentionModule;
#[doc(hidden)]
pub use modules::plan::KanbanModule;
#[doc(hidden)]
pub use modules::skill::SkillModule;
#[doc(hidden)]
pub use modules::delegation::DelegationModule;
#[doc(hidden)]
pub use modules::cron::CronModule;
#[doc(hidden)]
pub use modules::mcp::McpModule;
#[doc(hidden)]
pub use modules::tools::ToolsModule;
#[doc(hidden)]
pub use modules::webhook::WebhookModule;
#[doc(hidden)]
pub use modules::webhook::WebhookEvent;
#[doc(hidden)]
pub use modules::session::SessionModule;
pub mod scheduler;
pub use scheduler::{SessionScheduler, SessionTask};
pub use gateway::{GatewayRouter, CliGateway, InboundMessage, OutboundMessage};
pub use tremolite_session::SessionManager;
pub use module::{ModuleRegistry, Module, Event, EventContext, EventResponse, ModuleError, ModuleInfo, Capability, EngineHandle, ToolDefinition};
pub mod protocol;
pub use protocol::types::*;
pub use protocol::registry::ServiceRegistry;

/// 透闪石引擎
pub struct TremoliteEngine {
    pub providers: Arc<ProviderRegistry>,
    pub prompt_builder: PromptBuilder,
    pub tool_executor: Arc<dyn ToolExecutor + Send + Sync>,
    pub router: GatewayRouter,
    pub modules: ModuleRegistry,

    data_dir: PathBuf,
    pub base_soul: String,
    pub session_id: String,
}

impl TremoliteEngine {
    pub fn new(data_dir: PathBuf) -> Self {
        let modules = ModuleRegistry::new();
        Self {
            providers: Arc::new(ProviderRegistry::new()),
            prompt_builder: PromptBuilder::new("You are an AI assistant running on Tremolite."),
            tool_executor: Arc::new(CompositeToolExecutor {
                modules: modules.clone(),
                fallback: Box::new(NoopExecutor),
            }),

            router: GatewayRouter::new(),

            modules,

            data_dir,
            base_soul: "You are an AI assistant running on Tremolite.".into(),
            session_id: String::new(),
        }
    }

    /// 设置 LLM provider 并注册到模块系统（供 reflection 等模块通过 EngineHandle 访问）
    pub fn set_providers(&mut self, providers: Arc<ProviderRegistry>) {
        self.modules.set_providers(providers.clone());
        self.providers = providers;
    }

    /// 设置灵魂基底——从 SOUL.md 或 config 加载
    pub fn set_soul(&mut self, soul: &str) {
        self.base_soul = soul.to_string();
        self.prompt_builder.set_system_prompt(soul);
    }

    /// 从模块系统获取情绪显示文本
    pub fn emotion_display(&self) -> String {
        self.modules.get_display_status("emotion")
            .unwrap_or_else(|| "neutral".into())
    }

    pub fn register_module(&mut self, module: Box<dyn Module>) -> Result<(), ModuleError> {
        self.modules.register(module)
    }

    /// 注册 CLI 通道并启动
    pub fn start_cli(&mut self) -> Result<(), String> {
        self.router.register(Box::new(CliGateway::new()))
    }

    /// 创建 SessionScheduler 并启动后台调度线程
    ///
    /// 返回 (inbound_tx, outbound_rx)：
    /// - inbound_tx：向调度器投递消息的发送端
    /// - outbound_rx：接收调度器回复的接收端
    ///
    /// 调用前必须先调用 set_soul() 和 set_providers() 完成初始化。  
    /// CLI 模式直接调用 run()，内部自动调用此方法。  
    /// Daemon 模式调用此方法后将 inbound/outbound 桥接到消息通道。
    pub fn create_scheduler(&mut self) -> (
        std::sync::mpsc::Sender<scheduler::SessionTask>,
        std::sync::mpsc::Receiver<gateway::OutboundMessage>,
        std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, std::sync::mpsc::Sender<String>>>>,
    ) {
        // 启动事件（模块系统处理初始化）
        let ctx = EventContext::with_session(self.modules.handle(), self.session_id.clone());
        let _ = self.modules.broadcast(&Event::Startup, &ctx);

        // 创建 SessionScheduler
        let (mut scheduler, outbound_rx) = SessionScheduler::new(
            self.modules.clone(),
            self.providers.clone(),
            self.tool_executor.clone(),
            &self.base_soul,
        );
        let inbound_tx = scheduler.inbound();
        let pending_arc = scheduler.pending_results_arc();

        // 注入调度器通道到 DelegationModule（子 session 由此走调度器）
        let _ = self.modules.with_module_mut("delegation", |m| {
            if let Some(dm) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<crate::modules::delegation::DelegationModule>())
            {
                dm.set_scheduler(inbound_tx.clone(), pending_arc.clone());
            }
        });

        // 注入调度器通道到 CronModule（定时任务由此投递到调度器）
        let _ = self.modules.with_module_mut("cron", |m| {
            if let Some(cm) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<crate::modules::cron::CronModule>())
            {
                cm.set_scheduler(inbound_tx.clone());
            }
        });

        // 在后台线程运行调度器
        thread::spawn(move || {
            scheduler.run();
        });

        (inbound_tx, outbound_rx, pending_arc)
    }

    /// 运行主循环——阻塞，直到收到退出信号
    /// 使用 SessionScheduler 为每个 session 分配独立工作线程并行处理
    pub fn run(&mut self) {
        // 加载 SOUL.md——首次启动时从文件读取
        let soul_path = std::path::Path::new("SOUL.md");
        if self.base_soul == "You are an AI assistant running on Tremolite." && soul_path.exists() {
            if let Ok(content) = std::fs::read_to_string(soul_path) {
                let trimmed = content.trim().to_string();
                if !trimmed.is_empty() {
                    self.set_soul(&trimmed);
                    tracing::info!("engine: loaded SOUL.md ({} chars)", trimmed.len());
                }
            }
        }

        let (inbound_tx, outbound_rx, _pending) = self.create_scheduler();

        // 主循环：读取入站消息 → 转发给调度器
        // 每次循环先冲洗出站队列（非阻塞）
        loop {
            // 冲洗出站——worker 的回复不积压
            while let Ok(outbound) = outbound_rx.try_recv() {
                let _ = self.router.send(&outbound);
            }

            // 等待下一条入站消息
            let inbound = match self.router.recv() {
                Ok(msg) => msg,
                Err(_) => break,
            };

            let input = inbound.content.clone();
            let channel = inbound.channel.clone();
            let sender = inbound.sender.clone();

            if input == "exit" || input == "/quit" {
                self.shutdown();
                break;
            }

            // 转发给调度器——session_id = sender（每个用户/通道一个独立会话）
            let _ = inbound_tx.send(SessionTask {
                session_id: sender,
                input,
                channel,
                sender: inbound.sender,
            });
        }
    }

    /// 获取当前情绪显示文本<br>    pub fn emotion_display(&self) -> String {<br>        self.modules.get_display_status("emotion")<br>            .unwrap_or_else(|| "neutral".into())<br>    }<br><br>    pub fn register_module(&mut self, module: Box<dyn Module>) -> Result<(), ModuleError> {<br>        self.modules.register(module)<br>    }<br><br>    /// 注册 CLI 通道并启动<br>    pub fn start_cli(&mut self) -> Result<(), String> {<br>        self.router.register(Box::new(CliGateway::new()))<br>    }<br><br>    /// 创建 SessionScheduler 并启动后台调度线程<br>    ///<br>    /// 返回 (inbound_tx, outbound_rx, pending_results)：<br>    /// - inbound_tx：向调度器投递消息的发送端<br>    /// - outbound_rx：接收调度器回复的接收端<br>    /// - pending_results：子 session 回复映射表（HTTP handler 等用此同步等回复）<br>    ///<br>    /// 调用前必须先调用 set_soul() 和 set_providers() 完成初始化。  <br>    /// CLI 模式直接调用 run()，内部自动调用此方法。  <br>    /// Daemon 模式调用此方法后将 inbound/outbound 桥接到消息通道。

    fn shutdown(&mut self) {
        // 广播 Shutdown 事件——各模块自行 flush 持久化
        let ctx = EventContext::with_session(self.modules.handle(), self.session_id.clone());
        let _ = self.modules.broadcast(&Event::Shutdown, &ctx);
    }
}

struct NoopExecutor;
impl ToolExecutor for NoopExecutor {
    fn execute_tool(&self, name: &str, _args: &str) -> Result<String, String> {
        Err(format!("Tool '{}' not available (no executor configured)", name))
    }
    fn list_tools(&self) -> Vec<ToolDefinition> { Vec::new() }
}

struct WrapperExecutor<'a> {
    inner: &'a dyn ToolExecutor,
}
impl<'a> ToolExecutor for WrapperExecutor<'a> {
    fn execute_tool(&self, name: &str, args: &str) -> Result<String, String> {
        self.inner.execute_tool(name, args)
    }
    fn list_tools(&self) -> Vec<ToolDefinition> {
        self.inner.list_tools()
    }
}

/// 复合工具执行器——先查模块，未找到则回退旧执行器
pub struct CompositeToolExecutor {
    pub modules: ModuleRegistry,
    pub fallback: Box<dyn ToolExecutor>,
}
impl ToolExecutor for CompositeToolExecutor {
    fn execute_tool(&self, name: &str, args: &str) -> Result<String, String> {
        // 先查模块工具
        let mod_tools = self.modules.collect_tools();
        for (mod_id, def) in &mod_tools {
            if def.function.name == name {
                // 找到了——通过模块执行（execute_tool_on 内部用 Mutex 获取 &mut Module）
                return self.modules.execute_tool_on(mod_id, name, args)
                    .map_err(|e| e.to_string());
            }
        }
        // 未找到，走旧执行器
        self.fallback.execute_tool(name, args)
    }

    fn list_tools(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        // 旧执行器工具
        tools.extend(self.fallback.list_tools());
        // 模块工具
        for (_mod_id, def) in self.modules.collect_tools() {
            tools.push(def);
        }
        tools
    }
}
pub struct AttentionContributor {
    pub attention_data: std::sync::Mutex<String>,
}
impl PromptContributor for AttentionContributor {
    fn id(&self) -> &str { "attention" }
    fn priority(&self) -> u8 { 70 }
    fn contribute(&self, _ctx: &PromptContext) -> Option<String> {
        let data = self.attention_data.lock().ok()?;
        if data.is_empty() { None }
        else { Some(format!("[注意力扫描结果]\n以下内容基于当前对话的语义相关性分析：\n{}\n（注意力引擎：混合语义模型）", data)) }
    }
}

/// Prompt 贡献者：情绪插件
pub struct EmotionContributor {
    pub emotion_state: std::sync::Mutex<crate::modules::emotion::EmotionModule>,
}
impl PromptContributor for EmotionContributor {
    fn id(&self) -> &str { "emotion" }
    fn priority(&self) -> u8 { 80 }
    fn contribute(&self, _ctx: &PromptContext) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_creation() {
        let tmp = std::env::temp_dir().join("tremolite-engine-test");
        let engine = TremoliteEngine::new(tmp);
        assert!(engine.router.list_channels().is_empty());
    }
}
