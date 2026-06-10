use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use crate::protocol::types::{PowerCoupling, ModuleMessage, MessageEvent, ModuleDeclaration, ModuleAuthor, ServiceDefinition, ModuleHealth, ModuleStatus};
pub use tremolite_llm::{LLMProvider, ToolDefinition};

pub mod process_module;

// ─── 进程模块通信类型 ────────────────────────────

/// 进程模块的能力声明消息（第一行 JSON）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityDeclaration {
    pub msg_type: String,
    pub id: String,
    pub name: String,
    pub version: String,
    pub provides: Vec<Capability>,
    pub requires: Vec<Capability>,
    pub tools: Vec<serde_json::Value>,
    pub prompt_contributions: Vec<PromptContribution>,
}

/// Prompt 贡献段
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptContribution {
    pub content: String,
}

/// 引擎事件消息——从引擎发往进程模块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineEventMessage {
    pub msg_type: String,
    pub event: String,
    pub data: serde_json::Value,
    pub seq: u64,
}

/// 模块事件响应——从进程模块发回引擎
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleEventResponse {
    pub msg_type: String,
    pub status: String,
    pub data: serde_json::Value,
}

/// 工具调用消息——引擎请求进程模块执行工具
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallMessage {
    pub msg_type: String,
    pub name: String,
    pub args: serde_json::Value,
    pub tool_call_id: String,
}

/// 工具结果消息——进程模块返回工具执行结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub msg_type: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

/// 模块推送消息——进程模块主动向引擎推送数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulePushMessage {
    pub msg_type: String,
    pub push_type: String,
    pub data: serde_json::Value,
}

// ─── 能力标识 ─────────────────────────────────────

/// 能力标识符——模块能做什么的描述符
/// 命名规范：`<module_id>.<action>`
/// 示例：`emotion.detect`, `memory.recall`, `tool.file_read`
pub type Capability = String;

// ─── 事件 ─────────────────────────────────────────

/// 引擎生命周期事件——向所有模块广播
#[derive(Debug, Clone)]
pub enum Event {
    /// 引擎启动
    Startup,

    /// 引擎关闭
    Shutdown,

    /// 收到用户消息（LLM 调用前）
    /// Emotion 在此检测情绪
    /// Memory 在此存储用户输入
    /// Learning 在此练习技能
    OnMessage {
        input: String,
        channel: String,
    },

    /// 构建系统提示词阶段
    /// 引擎在此阶段收集所有模块的 prompt 贡献
    BuildPrompt,

    /// 工具被调用
    /// Learning 在此记录工具使用
    OnToolCall {
        name: String,
        args: String,
        success: bool,
    },

    /// LLM 生成回复后
    /// Memory 在此存储回复
    /// Learning 在此做学习建议
    OnResponse {
        response: String,
    },

    /// 新模块注册完成后
    /// 通知各模块（尤其是 Learning）发现新能力
    ModuleRegistered {
        info: ModuleInfo,
    },

    /// 触发记忆清洗（由 ReflectionModule 在反思后广播）
    /// MemoryModule 响应此事件执行虚构检测、噪音删除、去重合并
    Decontaminate,
}

/// 模块信息（用于 ModuleRegistered 事件和外部查询）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub provides: Vec<Capability>,
    pub requires: Vec<Capability>,
    pub tools: Vec<ToolDefinition>,
}

// ─── 事件响应 ────────────────────────────────────

/// 模块对事件的响应
#[derive(Debug)]
pub enum EventResponse {
    /// 正常处理，无事发生
    Pass,

    /// 模块要求跳过本轮后续处理
    Skip,

    /// 模块修改了共享数据
    Modified {
        data: HashMap<String, Box<dyn Any + Send>>,
    },
}

// ─── 模块错误 ────────────────────────────────────

#[derive(Debug)]
pub enum ModuleError {
    InitFailed(String),
    EventFailed(String),
    ToolNotFound(String),
    ToolExecutionFailed(String),
    ShutdownFailed(String),
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InitFailed(msg) => write!(f, "init failed: {msg}"),
            Self::EventFailed(msg) => write!(f, "event failed: {msg}"),
            Self::ToolNotFound(msg) => write!(f, "tool not found: {msg}"),
            Self::ToolExecutionFailed(msg) => write!(f, "tool execution failed: {msg}"),
            Self::ShutdownFailed(msg) => write!(f, "shutdown failed: {msg}"),
        }
    }
}

impl std::error::Error for ModuleError {}

// ─── 模块上下文 ──────────────────────────────────

/// 模块上下文——事件广播时提供给模块的临时数据通道
pub struct EventContext {
    /// 会话 ID——空字符串表示默认会话
    pub session_id: String,

    /// 当前事件的额外数据
    /// 模块可以通过此通道传递数据给后续阶段的模块
    pub data: HashMap<String, Box<dyn Any + Send>>,

    /// 引擎句柄——模块通过它访问其他模块
    pub engine: EngineHandle,
}

impl EventContext {
    pub fn new(engine: EngineHandle) -> Self {
        Self {
            session_id: String::new(),
            data: HashMap::new(),
            engine,
        }
    }

    pub fn with_session(engine: EngineHandle, session_id: String) -> Self {
        Self {
            session_id,
            data: HashMap::new(),
            engine,
        }
    }
}

/// 引擎句柄——模块通过它访问其他模块的公开数据
/// 内部持有 ModuleRegistry 的弱引用，防止循环引用
#[derive(Clone)]
pub struct EngineHandle {
    inner: Weak<Mutex<ModuleRegistryInner>>,
}

impl EngineHandle {
    pub fn new(inner: Weak<Mutex<ModuleRegistryInner>>) -> Self {
        Self { inner }
    }

    /// 通过闭包访问指定模块（可读可写，利用 as_any 向下转型）
    /// 调用方通过类型参数 T 指定具体模块类型
    pub fn with_module<R>(&self, module_id: &str, f: impl FnOnce(&mut dyn Module) -> R) -> Option<R> {
        let registry = self.inner.upgrade()?;
        let mut guard = registry.lock().ok()?;
        guard.modules.get_mut(module_id).map(|m| f(&mut **m))
    }

    /// 按能力查询拥有该能力的模块 ID
    pub fn find_by_capability(&self, cap: &Capability) -> Option<String> {
        let registry = self.inner.upgrade()?;
        let guard = registry.lock().ok()?;
        guard.capability_index.get(cap).cloned()
    }

    /// 向所有模块广播事件（异步推事件到注册表）
    pub fn broadcast(&self, event: &Event, ctx: &EventContext) -> Vec<(String, Result<EventResponse, ModuleError>)> {
        let registry = match self.inner.upgrade() {
            Some(r) => r,
            None => return Vec::new(),
        };
        let ids: Vec<String> = match registry.lock() {
            Ok(inner) => inner.order.clone(),
            Err(_) => return Vec::new(),
        };

        let mut results = Vec::new();
        for id in &ids {
            let result = match registry.lock() {
                Ok(mut inner) => match inner.modules.get_mut(id) {
                    Some(module) => module.on_event(event, ctx),
                    None => Err(ModuleError::EventFailed(format!("module '{}' not found", id))),
                },
                Err(e) => Err(ModuleError::EventFailed(format!("lock failed: {e}"))),
            };
            results.push((id.clone(), result));
        }
        results
    }

    /// 获取某模块的 ModuleRef（只读访问权柄）
    pub fn get_module(&self, id: &str) -> Option<ModuleRef> {
        let registry = self.inner.upgrade()?;
        let guard = registry.lock().ok()?;
        if guard.modules.contains_key(id) {
            Some(ModuleRef {
                module_id: id.to_string(),
                inner: self.inner.clone(),
            })
        } else {
            None
        }
    }

    /// 获取所有已注册模块的信息列表
    pub fn list_modules(&self) -> Vec<ModuleInfo> {
        let registry = match self.inner.upgrade() {
            Some(r) => r,
            None => return Vec::new(),
        };
        let guard = match registry.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        guard.infos.values().cloned().collect()
    }

    /// 按模块 ID 查询其对外暴露的数据
    /// 返回 `None` 表示模块不存在或数据不可用
    /// 按模块 ID 查询其对外暴露的数据
    pub fn query_module_raw_data(&self, module_id: &str) -> Option<*const (dyn Any + Send)> {
        let registry = self.inner.upgrade()?;
        let guard = registry.lock().ok()?;
        let ptr: &Box<dyn Any + Send> = guard.module_data.get(module_id)?;
        let raw: *const (dyn Any + Send) = &**ptr;
        Some(raw)
    }

    /// 按任务类型获取 provider 名称（通过模型路由表）
    /// 未注册的任务类型返回 None，由调用方回退到默认 provider
    pub fn get_provider_for(&self, task: &str) -> Option<String> {
        let registry = self.inner.upgrade()?;
        let guard = registry.lock().ok()?;
        let name = guard.model_router.provider_name(task)?;
        Some(name.to_string())
    }

    /// 获取 LLM Provider 注册表
    pub fn get_providers(&self) -> Option<std::sync::Arc<tremolite_llm::ProviderRegistry>> {
        let registry = self.inner.upgrade()?;
        let guard = registry.lock().ok()?;
        guard.providers.clone()
    }
}

/// 模块引用——模模块获得其他模块的只读访问
pub struct ModuleRef {
    module_id: String,
    inner: Weak<Mutex<ModuleRegistryInner>>,
}

impl ModuleRef {
    pub fn id(&self) -> &str {
        &self.module_id
    }

    pub fn info(&self) -> Option<ModuleInfo> {
        let registry = self.inner.upgrade()?;
        let guard = registry.lock().ok()?;
        guard.infos.get(&self.module_id).cloned()
    }
}

// ─── Module trait ─────────────────────────────────

/// 统一模块接口
///
/// 情绪、记忆、注意力、学习、计划书、外部插件……
/// 透闪石的一切功能模块都通过此接口与引擎交互。
///
/// # 可选方法
/// `prompt_segment()` 默认返回 None（不贡献 prompt）
/// `tool_definitions()` 默认返回空（不提供工具）
/// `on_event()` 默认返回 Pass（不处理事件）
pub trait Module: Send + Sync {
    // ─── 元数据 ────────────────────────────

    /// 模块唯一标识，如 "emotion", "memory", "qqbot"
    fn id(&self) -> &str;

    /// 人类可读名称，如 "情绪引擎"
    fn name(&self) -> &str;

    /// 语义版本号
    fn version(&self) -> &str;

    // ─── 能力声明 ──────────────────────────

    /// 本模块提供的能力列表
    fn provides(&self) -> Vec<Capability>;

    /// 本模块依赖的能力列表
    fn requires(&self) -> Vec<Capability>;

    /// 本模块依赖的其他模块 ID 列表
    ///
    /// 如果依赖的模块未注册，注册时会返回 InitFailed 错误，
    /// 提示用户安装缺少的模块。默认不依赖任何模块。
    fn required_modules(&self) -> Vec<&str> {
        Vec::new()
    }

    // ─── 生命周期 ──────────────────────────

    /// 初始化模块
    fn init(&mut self, _ctx: &EventContext) -> Result<(), ModuleError> {
        Ok(())
    }

    /// 关闭模块
    fn shutdown(&mut self) -> Result<(), ModuleError> {
        Ok(())
    }

    // ─── Prompt 贡献（可选） ────────────────

    /// 返回本模块要注入到 system prompt 的内容
    /// 默认不贡献任何内容。需要贡献的模块覆写此方法。
    fn prompt_segment(&self) -> Option<String> {
        None
    }

    /// 返回TUI状态栏显示的紧凑状态文本（可选）
    /// 默认返回 None，不参与状态栏显示。
    fn display_status(&self) -> Option<String> {
        None
    }

    // ─── 工具（可选） ──────────────────────

    /// 本模块提供的工具定义
    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        Vec::new()
    }

    /// 执行工具
    fn execute_tool(&mut self, _name: &str, _args: &str) -> Result<String, ModuleError> {
        Err(ModuleError::ToolNotFound("no tools".into()))
    }

    // ─── 事件响应（可选） ──────────────────

    /// 响应引擎生命周期事件
    fn on_event(&mut self, _event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        Ok(EventResponse::Pass)
    }

    // ─── 类型化访问（可选，用于从 registry 获取具体模块） ──

    /// 返回 &dyn Any 引用，用于向下转型到具体模块类型
    /// 默认返回 None（不支持转型）。需要被外部按类型访问的模块覆写此方法。
    fn as_any(&self) -> Option<&dyn Any> { None }

    /// 返回 &mut dyn Any 引用，用于向下转型到具体模块类型
    /// 默认返回 None（不支持转型）。
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        None
    }

    // ─── 动力耦合器（可选） ─────────────────

    /// 设置模块的动力耦合器——模块通过它接入引擎，获取各种服务。
    /// 引擎在注册模块后调用。
    fn set_coupling(&mut self, _coupling: PowerCoupling) {}

    /// 处理收到的消息（引擎路由后调用）。
    /// 返回要发送的消息列表（回复、新请求等）。
    /// 默认不处理任何消息。
    fn on_message(&mut self, _event: MessageEvent) -> Result<Vec<ModuleMessage>, ModuleError> {
        Ok(Vec::new())
    }

    /// 获取模块的声明（作者、能力、依赖、消息处理器列表）。
    /// 返回 Some 表示该模块支持模块通讯协议。
    /// 返回 None 表示该模块仍使用旧的 direct-access 模式。
    fn declaration(&self) -> Option<ModuleDeclaration> {
        None
    }

    /// 获取模块健康状态。
    fn health(&self) -> ModuleHealth {
        ModuleHealth {
            id: self.id().to_string(),
            name: self.name().to_string(),
            version: self.version().to_string(),
            status: ModuleStatus::Running,
            message_count: 0,
            error_count: 0,
            uptime_secs: 0,
            services: self.provides(),
            dependencies: self.requires(),
            last_error: None,
            details: HashMap::new(),
        }
    }
}

// ─── 模型路由 ────────────────────────────────────

/// 模型路由表——按任务类型分配不同的 provider
///
/// 模块在需要调用 LLM 时，先查此表按任务类型获取 provider。
/// 未注册的任务类型回退到默认 provider。
#[derive(Clone)]
pub struct ModelRouter {
    routes: HashMap<String, String>,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self { routes: HashMap::new() }
    }

    /// 注册一条路由：任务类型 → provider 名称
    pub fn register(&mut self, task: &str, provider: &str) {
        self.routes.insert(task.to_string(), provider.to_string());
    }

    /// 查询任务类型对应的 provider 名称
    pub fn provider_name(&self, task: &str) -> Option<&str> {
        self.routes.get(task).map(|s| s.as_str())
    }
}

// ─── ModuleRegistry inner ─────────────────────────

/// 模块注册表内部数据
pub struct ModuleRegistryInner {
    pub modules: HashMap<String, Box<dyn Module>>,
    pub infos: HashMap<String, ModuleInfo>,
    pub capability_index: HashMap<String, String>, // capability -> module_id
    pub module_data: HashMap<String, Box<dyn Any + Send>>,
    pub order: Vec<String>,
    pub providers: Option<std::sync::Arc<tremolite_llm::ProviderRegistry>>,
    /// 模型路由表——任务类型 → provider 名称
    pub model_router: ModelRouter,
}

/// 统一模块注册表
#[derive(Clone)]
pub struct ModuleRegistry {
    inner: Arc<Mutex<ModuleRegistryInner>>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ModuleRegistryInner {
                modules: HashMap::new(),
                infos: HashMap::new(),
                capability_index: HashMap::new(),
                module_data: HashMap::new(),
                order: Vec::new(),
                providers: None,
                model_router: ModelRouter::new(),
            })),
        }
    }

    /// 创建引擎句柄（给模块的 EventContext 用）
    pub fn handle(&self) -> EngineHandle {
        EngineHandle::new(Arc::downgrade(&self.inner))
    }

    /// 注册一个模块
    pub fn register(&mut self, module: Box<dyn Module>) -> Result<(), ModuleError> {
        let id = module.id().to_string();
        let name = module.name().to_string();
        let version = module.version().to_string();
        let provides = module.provides();
        let requires = module.requires();
        let tools = module.tool_definitions();
        let required_modules = module.required_modules();

        let info = ModuleInfo {
            id: id.clone(),
            name,
            version,
            provides: provides.clone(),
            requires: requires.clone(),
            tools,
        };

        {
            let mut inner = self.inner.lock().map_err(|e| {
                ModuleError::InitFailed(format!("lock failed: {e}"))
            })?;

            // 检查能力依赖是否满足
            for req in &requires {
                if !inner.capability_index.contains_key(req) {
                    return Err(ModuleError::InitFailed(
                        format!("module '{}' requires capability '{}' which is not provided by any registered module", id, req)
                    ));
                }
            }

            // 检查模块级依赖是否满足
            for dep in &required_modules {
                if !inner.modules.contains_key(*dep) {
                    return Err(ModuleError::InitFailed(
                        format!(
                            "module '{}' requires module '{}' which is not registered. \
                             Please add the '{}' module to your module list.",
                            id, dep, dep
                        )
                    ));
                }
            }

            // 注册能力
            for cap in &provides {
                inner.capability_index.insert(cap.clone(), id.clone());
            }

            inner.infos.insert(id.clone(), info.clone());
            inner.modules.insert(id.clone(), module);
            inner.order.push(id.clone());
        }

        // 广播 ModuleRegistered 事件
        let ctx = EventContext::new(self.handle());
        self.broadcast(&Event::ModuleRegistered { info: info.clone() }, &ctx);

        tracing::info!("module: registered '{}'", id);
        Ok(())
    }

    /// 注册模块对外暴露的数据（用于模块间通信）
    pub fn set_module_data<T: Send + 'static>(&self, module_id: &str, data: T) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.module_data.insert(module_id.to_string(), Box::new(data));
        }
    }

    /// 设置 LLM Provider 注册表（供 reflection 等模块通过 EngineHandle 访问）
    pub fn set_providers(&self, providers: std::sync::Arc<tremolite_llm::ProviderRegistry>) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.providers = Some(providers);
        }
    }

    /// 重新初始化所有模块的顺序
    pub fn init_all(&mut self, ctx: &EventContext) -> Result<(), ModuleError> {
        let ids: Vec<String> = {
            let inner = self.inner.lock().map_err(|e| {
                ModuleError::InitFailed(format!("lock failed: {e}"))
            })?;
            inner.order.clone()
        };

        for id in &ids {
            let mut inner = self.inner.lock().map_err(|e| {
                ModuleError::InitFailed(format!("lock failed: {e}"))
            })?;
            if let Some(module) = inner.modules.get_mut(id) {
                module.init(ctx)?;
            }
        }
        Ok(())
    }

    /// 向所有模块广播事件
    pub fn broadcast(&self, event: &Event, ctx: &EventContext) -> Vec<(String, Result<EventResponse, ModuleError>)> {
        let ids: Vec<String> = {
            match self.inner.lock() {
                Ok(inner) => inner.order.clone(),
                Err(_) => return Vec::new(),
            }
        };

        let mut results = Vec::new();
        for id in &ids {
            let result = match self.inner.lock() {
                Ok(mut inner) => {
                    match inner.modules.get_mut(id) {
                        Some(module) => module.on_event(event, ctx),
                        None => Err(ModuleError::EventFailed(format!("module '{}' not found", id))),
                    }
                }
                Err(e) => Err(ModuleError::EventFailed(format!("lock failed: {e}"))),
            };
            results.push((id.clone(), result));
        }
        results
    }

    /// 获取指定模块的状态栏显示文本
    pub fn get_display_status(&self, module_id: &str) -> Option<String> {
        match self.inner.lock() {
            Ok(inner) => {
                inner.modules.get(module_id)
                    .and_then(|m| m.display_status())
            }
            Err(_) => None,
        }
    }

    /// 收集所有模块的 prompt 贡献
    pub fn collect_prompt_segments(&self) -> Vec<(String, String)> {
        let mut segments = Vec::new();
        match self.inner.lock() {
            Ok(inner) => {
                for id in &inner.order {
                    if let Some(module) = inner.modules.get(id) {
                        if let Some(segment) = module.prompt_segment() {
                            segments.push((id.clone(), segment));
                        }
                    }
                }
            }
            Err(_) => {}
        }
        segments
    }

    /// 收集所有模块的工具定义
    pub fn collect_tools(&self) -> Vec<(String, ToolDefinition)> {
        let mut tools = Vec::new();
        match self.inner.lock() {
            Ok(inner) => {
                for id in &inner.order {
                    if let Some(module) = inner.modules.get(id) {
                        for tool in module.tool_definitions() {
                            tools.push((id.clone(), tool));
                        }
                    }
                }
            }
            Err(_) => {}
        }
        tools
    }

    /// 按模块 ID 获取模块，通过闭包访问
    /// 适用于需要按具体类型访问模块的场景（如获取 MemoryModule 的统计数据）
    pub fn with_module<R>(&self, module_id: &str, f: impl FnOnce(&dyn Module) -> R) -> Option<R> {
        match self.inner.lock() {
            Ok(inner) => inner.modules.get(module_id).map(|m| f(&**m)),
            Err(_) => None,
        }
    }

    /// 按模块 ID 获取模块，通过闭包访问（可变）
    pub fn with_module_mut<R>(
        &self,
        module_id: &str,
        f: impl FnOnce(&mut dyn Module) -> R,
    ) -> Option<R> {
        let mut inner = match self.inner.lock() {
            Ok(i) => i,
            Err(_) => return None,
        };
        inner.modules.get_mut(module_id).map(|m| f(&mut **m))
    }

    /// 注册模型路由：任务类型 → provider 名称
    /// 模块调用 handle.get_provider_for("task") 时，会按此表查找
    pub fn register_model_route(&self, task: &str, provider: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.model_router.register(task, provider);
        }
    }

    /// 按模块 ID 执行工具（外部调用此方法，自行管理锁）
    pub fn execute_tool_on(&self, module_id: &str, name: &str, args: &str) -> Result<String, ModuleError> {
        let mut inner = self.inner.lock().map_err(|e| {
            ModuleError::ToolExecutionFailed(format!("lock failed: {e}"))
        })?;

        match inner.modules.get_mut(module_id) {
            Some(module) => module.execute_tool(name, args),
            None => Err(ModuleError::ToolNotFound(format!("module '{module_id}' not found"))),
        }
    }

    /// 检查某个能力是否已被注册
    pub fn has_capability(&self, capability: &str) -> bool {
        match self.inner.lock() {
            Ok(inner) => inner.capability_index.contains_key(capability),
            Err(_) => false,
        }
    }

    /// 调用模块的 shutdown 并清理资源
    pub fn shutdown_all(&mut self) -> Vec<(String, Result<(), ModuleError>)> {
        let ids: Vec<String> = {
            match self.inner.lock() {
                Ok(inner) => inner.order.clone(),
                Err(_) => return Vec::new(),
            }
        };

        let mut results = Vec::new();
        for id in ids.iter().rev() {
            let result = match self.inner.lock() {
                Ok(mut inner) => {
                    match inner.modules.get_mut(id) {
                        Some(module) => module.shutdown(),
                        None => Ok(()),
                    }
                }
                Err(e) => Err(ModuleError::ShutdownFailed(format!("lock failed: {e}"))),
            };
            results.push((id.clone(), result));
        }
        results
    }
}
