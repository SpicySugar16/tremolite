/// Session 调度器——多 session 并发处理
///
/// 将消息路由到正确的 SessionWorker 线程，支持多个 session 独立并行处理。
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tremolite_llm::{
    LLMProvider, Message, PromptBuilder, PromptContext, PromptContributor, ProviderRegistry,
    ToolCallLoop, ToolExecutor,
};

use crate::gateway::OutboundMessage;
use crate::module::{Event, EventContext, EventResponse, ModuleRegistry};

/// 调度器收到的任务
pub struct SessionTask {
    pub session_id: String,
    pub input: String,
    pub channel: String,
    pub sender: String,
}

/// Session 调度器
pub struct SessionScheduler {
    /// 工作线程表
    workers: HashMap<String, SessionWorkerHandle>,
    /// worker 的入站发送端
    worker_channels: HashMap<String, mpsc::Sender<SessionTask>>,
    /// 入站通道（外部往这里发消息）
    inbound_tx: mpsc::Sender<SessionTask>,
    /// 入站接收端（调度器内部使用）
    inbound_rx: mpsc::Receiver<SessionTask>,
    /// 出站通道（worker 往这里写回复）
    outbound_tx: mpsc::Sender<OutboundMessage>,
    /// 待返回结果（子 session 用）—— session_id → 回复通道
    pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    /// 共享状态
    shared: Arc<SchedulerShared>,
    /// worker 空闲超时——超过此时间无消息则自动退出（默认 300 秒）
    idle_timeout: Duration,
    /// worker 死亡通知通道——panic 的 worker 往这里发 session_id
    death_tx: mpsc::Sender<String>,
    /// 死亡通知接收端——调度器主循环从中读取已死亡的 worker
    worker_deaths: mpsc::Receiver<String>,
}

/// 所有 worker 共享的状态
pub struct SchedulerShared {
    pub modules: ModuleRegistry,
    pub providers: Arc<ProviderRegistry>,
    pub executor: Arc<dyn ToolExecutor + Send + Sync>,
    pub base_soul: String,
}

struct SessionWorkerHandle {
    session_id: String,
    _thread: thread::JoinHandle<()>,
}

/// 单个 session 的工作线程
struct SessionWorker {
    session_id: String,
    base_soul: String,
    prompt_builder: PromptBuilder,
    modules: ModuleRegistry,
    providers: Arc<ProviderRegistry>,
    executor: Arc<dyn ToolExecutor + Send + Sync>,
    outbound_tx: mpsc::Sender<OutboundMessage>,
    inbound_rx: mpsc::Receiver<SessionTask>,
    pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    idle_timeout: Duration,
    last_active: Instant,
    death_tx: mpsc::Sender<String>,
}

impl SessionWorker {
    fn new(
        session_id: String,
        base_soul: &str,
        modules: ModuleRegistry,
        providers: Arc<ProviderRegistry>,
        executor: Arc<dyn ToolExecutor + Send + Sync>,
        outbound_tx: mpsc::Sender<OutboundMessage>,
        inbound_rx: mpsc::Receiver<SessionTask>,
        pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
        idle_timeout: Duration,
        death_tx: mpsc::Sender<String>,
    ) -> Self {
        Self {
            session_id,
            base_soul: base_soul.to_string(),
            prompt_builder: PromptBuilder::new(base_soul),
            modules,
            providers,
            executor,
            outbound_tx,
            inbound_rx,
            pending_results,
            idle_timeout,
            last_active: Instant::now(),
            death_tx,
        }
    }

    /// 运行消息循环——独立线程中执行
    fn run(mut self) {
        let sid = self.session_id.clone();
        let death_tx = self.death_tx.clone();

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.inner_run();
        }));

        match result {
            Ok(()) => {
                tracing::info!("scheduler: worker stopped for session '{}'", sid);
            }
            Err(panic_info) => {
                let msg = panic_info
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| panic_info.downcast_ref::<String>().map(|s| s.clone()))
                    .unwrap_or_else(|| "unknown panic".to_string());
                tracing::error!("scheduler: worker PANICKED for session '{}': {}", sid, msg);
                let _ = death_tx.send(sid);
            }
        }
    }

    /// 实际消息循环——被 run() 的 catch_unwind 包裹
    fn inner_run(&mut self) {
        tracing::info!("scheduler: worker started for session '{}'", self.session_id);

        // 广播 Startup 事件
        let ctx = EventContext::with_session(self.modules.handle(), self.session_id.clone());
        let _ = self.modules.broadcast(&Event::Startup, &ctx);

        // 消息循环——使用 recv_timeout 支持空闲退出
        loop {
            let task = match self.inbound_rx.recv_timeout(self.idle_timeout) {
                Ok(task) => {
                    self.last_active = Instant::now();
                    task
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // 空闲超时——检查是否真的空闲了 idle_timeout 这么久
                    if self.last_active.elapsed() >= self.idle_timeout {
                        tracing::info!("scheduler: worker idle timeout for session '{}'", self.session_id);
                        break;
                    }
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };

            let input = task.input;
            let channel = task.channel;
            let sender = task.sender;
            tracing::info!(
                "scheduler: message received by worker '{}': input='{}', channel='{}', sender='{}'",
                self.session_id, input, channel, sender
            );

            // 模块处理：情绪检测、记忆写入等
            tracing::debug!("scheduler: broadcasting OnMessage event");
            let ctx = EventContext::with_session(self.modules.handle(), self.session_id.clone());
            let _ = self.modules.broadcast(&Event::OnMessage {
                input: input.clone(),
                channel: channel.clone(),
            }, &ctx);
            tracing::debug!("scheduler: OnMessage broadcast complete");

            // BuildPrompt
            tracing::debug!("scheduler: broadcasting BuildPrompt event");
            let _ = self.modules.broadcast(&Event::BuildPrompt, &ctx);
            tracing::debug!("scheduler: BuildPrompt broadcast complete");

            // 检查是否是斜杠命令——不走 LLM，直接响应
            let response = if input.starts_with('/') {
                self.handle_command(&input, &channel)
            } else {
                self.process(&input, &channel)
            };

            // 回复写入记忆
            let _ = self.modules.broadcast(&Event::OnResponse {
                response: response.clone(),
            }, &ctx);

            // 检查是否有父 session 在等这个子 session 的结果
            // 用 task.sender（即 pending_id）匹配，而不是 self.session_id
            let delivered_to_pending = if let Ok(mut map) = self.pending_results.lock() {
                if let Some(tx) = map.remove(&sender) {
                    let _ = tx.send(response.clone());
                    true
                } else { false }
            } else { false };

            // 如果已经通过 pending_results 送达（如 HTTP 请求），跳过出站通道
            if !delivered_to_pending {
                // 发送回复到出站队列
                let outbound = OutboundMessage::new(&response, &channel, &sender);
            tracing::debug!(
                "scheduler: sending outbound to channel '{}', target '{}': {} chars",
                channel, sender, response.len()
            );
            if let Err(e) = self.outbound_tx.send(outbound) {
                tracing::error!(
                    "scheduler: outbound send failed for session '{}': {}",
                    self.session_id, e
                );
            }
            }
        }

        // 广播 Shutdown 事件，通知模块清理
        let ctx = EventContext::with_session(self.modules.handle(), self.session_id.clone());
        let _ = self.modules.broadcast(&Event::Shutdown, &ctx);
    }

    /// 处理单轮对话（从 process_with_llm 移植）
    fn process(&mut self, input: &str, _channel: &str) -> String {
        tracing::info!("process step 1: collecting prompt segments");
        let module_segments: Vec<String> = self.modules.collect_prompt_segments()
            .into_iter()
            .map(|(_id, segment)| segment)
            .collect();

        let mut prompt_parts = vec![self.base_soul.clone()];
        prompt_parts.extend(module_segments);
        let full_prompt = prompt_parts.join("\n\n");
        self.prompt_builder.set_system_prompt(&full_prompt);

        tracing::info!("process step 2: listing tools");
        let all_tools = self.executor.list_tools();
        let available_tools: Vec<String> = all_tools.iter()
            .map(|t| t.function.name.clone())
            .collect();

        // 从 MemoryModule 获取本 session 的历史
        tracing::info!("process step 3: getting history");
        let mut history: Vec<Message> = self.modules.with_module("memory", |m| {
            m.as_any()
                .and_then(|any| any.downcast_ref::<crate::modules::memory::MemoryModule>())
                .map(|mm| {
                    mm.recent_entries(&self.session_id, 20).iter().filter_map(|entry| {
                        let c = &entry.content;
                        if let Some(user_msg) = c.strip_prefix("kamisama: ") {
                            Some(Message::user(user_msg))
                        } else if let Some(assistant_msg) = c.strip_prefix("葵: ") {
                            Some(Message::assistant(assistant_msg))
                        } else { None }
                    }).collect::<Vec<Message>>()
                })
                .unwrap_or_default()
        }).unwrap_or_default();

        // 步骤 3.5：预取内存条目内容序列化后喂给压缩模块
        // 先拿条目（释放内存锁），再通过 execute_tool_on 传给 compress
        // execute_tool_on 持有模块锁，但 compress_from_entries 不碰 memory
        let raw_entries: Vec<String> = self
            .modules
            .with_module("memory", |m| {
                m.as_any()
                    .and_then(|a| a.downcast_ref::<crate::modules::memory::MemoryModule>())
                    .map(|mm| {
                        mm.recent_entries("", 100)
                            .iter()
                            .map(|e| e.content.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        if !raw_entries.is_empty() {
            let entries_json = serde_json::json!({
                "entries": raw_entries.iter().map(|c| serde_json::json!({ "content": c })).collect::<Vec<_>>()
            });
            let _ = self
                .modules
                .execute_tool_on("compress", "compress_from_entries", &entries_json.to_string());
        }

        let ctx = PromptContext {
            user_input: input.to_string(),
            conversation_history: history,
            available_tools,
        };
        let messages = self.prompt_builder.build(&ctx);
        tracing::info!(
            "scheduler: process calling LLM with {} messages for session '{}'",
            messages.len(), self.session_id
        );

        if let Some(provider) = self.providers.get_default() {
            let tool_loop = ToolCallLoop::new();
            let executor: &dyn ToolExecutor = &*self.executor;
            let result = tool_loop.run(provider, &messages, executor);
            match result {
                Ok(result) => {
                    // 记录工具调用（通过模块）
                    let _ = self.modules.with_module_mut("skill", |m| {
                        for record in &result.call_history {
                            if let Some(sm) = m.as_any_mut()
                                .and_then(|any| any.downcast_mut::<crate::modules::skill::SkillModule>())
                            {
                                sm.engine_mut().practice("use_tool", record.success, &record.tool_name);
                            }
                        }
                    });
                    return result.content;
                }
                Err(e) => return format!("[LLM Error: {}]", e),
            }
        }

        // 无 LLM provider 时的 fallback
        let emotion = self.modules.with_module("emotion", |m| {
            m.as_any()
                .and_then(|any| any.downcast_ref::<crate::modules::emotion::EmotionModule>())
                .map(|em| em.composite_emotion())
                .unwrap_or_default()
        }).unwrap_or_default();
        match emotion.as_str() {
            "爱" | "快乐" | "欣喜" => "噜噜……神大人说的呢，葵听到了喔~".into(),
            "悲伤" | "焦虑" => "呜……神大人说这样的话，葵有点担心呢……".into(),
            "愤怒" | "不满" => "哼~神大人这样说葵可不高兴呢……".into(),
            _ => "噜噜……神大人说的葵听到了，葵正在努力理解喔~".into(),
        }
    }
}

impl SessionWorker {
    /// 处理斜杠命令——不走 LLM，直接从模块获取数据响应
    fn handle_command(&mut self, input: &str, channel: &str) -> String {
        let trimmed = input.trim();
        let parts: Vec<&str> = trimmed.splitn(2, |c: char| c == ' ' || c == '\t').collect();
        let cmd = parts[0];
        let args = parts.get(1).copied().unwrap_or("");

        match cmd {
            "/help" | "/h" | "/commands" => {
                let lines = [
                    "可用命令：",
                    "  /help        — 显示此帮助",
                    "  /new /reset  — 重置当前会话",
                    "  /title <名>   — 设置会话名称",
                    "  /model       — 查看当前模型",
                    "  /model <名>   — 切换 provider",
                    "  /models      — 列出所有已注册 provider",
                    "  /sessions    — 列出所有活跃会话",
                    "  /peek <id>   — 查看其他会话的上下文",
                    "  /share       — 共享当前会话",
                    "  /unshare     — 取消共享",
                    "  /status      — 当前会话状态",
                    "  /cron        — 列出定时任务",
                    "  /quit /exit  — 退出",
                ];
                lines.join("\n")
            }

            "/new" | "/reset" => {
                self.modules.with_module_mut("session", |m| {
                    if let Some(sm) = m.as_any_mut()
                        .and_then(|a| a.downcast_mut::<crate::modules::session::SessionModule>())
                    {
                        if let Some(state) = sm.manager.sessions_mut().get_mut(&self.session_id) {
                            state.close();
                        }
                    }
                });
                "会话已重置。可以重新开始了呢~".into()
            }

            "/title" => {
                let name = args.trim();
                if name.is_empty() {
                    return "用法：/title <会话名称>".into();
                }
                self.modules.with_module_mut("memory", |m| {
                    if let Some(mm) = m.as_any_mut()
                        .and_then(|a| a.downcast_mut::<crate::modules::memory::MemoryModule>())
                    {
                        mm.manager_mut().remember(
                            &self.session_id,
                            format!("[session_title] {}", name),
                            vec!["meta".into(), "title".into()],
                            0.9, "system".into(),
                        );
                    }
                });
                format!("会话标题已设为「{}」", name)
            }

            "/sessions" => {
                let list: Vec<String> = self.modules.with_module("session", |m| {
                    m.as_any()
                        .and_then(|a| a.downcast_ref::<crate::modules::session::SessionModule>())
                        .map(|sm| {
                            sm.manager.sessions().iter().map(|(id, state)| {
                                let shared = if state.shared { " [共享]" } else { "" };
                                format!("  {}{}", id, shared)
                            }).collect()
                        })
                        .unwrap_or_default()
                }).unwrap_or_default();

                if list.is_empty() {
                    "没有活跃会话喔~".into()
                } else {
                    let mut out = format!("活跃会话（{}）：", list.len());
                    for entry in list {
                        out.push_str(&format!("\n{}", entry));
                    }
                    out
                }
            }

            "/peek" => {
                let target = args.trim();
                if target.is_empty() {
                    return "用法：/peek <session_id>".into();
                }
                let history = self.modules.with_module("memory", |m| {
                    m.as_any()
                        .and_then(|a| a.downcast_ref::<crate::modules::memory::MemoryModule>())
                        .map(|mm| {
                            mm.recent_entries(target, 5).iter()
                                .map(|e| format!("  {}", e.content))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                }).unwrap_or_default();

                if history.is_empty() {
                    format!("会话「{}」没有找到记录呢", target)
                } else {
                    let mut out = format!("会话「{}」最近记录：", target);
                    for line in history {
                        out.push_str(&format!("\n{}", line));
                    }
                    out
                }
            }

            "/share" => {
                self.modules.with_module_mut("session", |m| {
                    if let Some(sm) = m.as_any_mut()
                        .and_then(|a| a.downcast_mut::<crate::modules::session::SessionModule>())
                    {
                        if let Some(state) = sm.manager.sessions_mut().get_mut(&self.session_id) {
                            state.share();
                        }
                    }
                });
                "已共享当前会话，别的 session 可以 peek 进来了喔".into()
            }

            "/unshare" => {
                self.modules.with_module_mut("session", |m| {
                    if let Some(sm) = m.as_any_mut()
                        .and_then(|a| a.downcast_mut::<crate::modules::session::SessionModule>())
                    {
                        if let Some(state) = sm.manager.sessions_mut().get_mut(&self.session_id) {
                            state.unshare();
                        }
                    }
                });
                "已取消共享".into()
            }

            "/status" => {
                let mut info = format!("会话：{}", self.session_id);
                self.modules.with_module("session", |m| {
                    if let Some(sm) = m.as_any()
                        .and_then(|a| a.downcast_ref::<crate::modules::session::SessionModule>())
                    {
                        if let Some(state) = sm.manager.sessions().get(&self.session_id) {
                            info.push_str(&format!(
                                "\n共享：{}\n已冷却：{}",
                                if state.shared { "是" } else { "否" },
                                if state.closed { "是" } else { "否" },
                            ));
                        }
                    }
                });
                if let Some(p) = self.providers.get_default() {
                    info.push_str(&format!("\n模型：{}", p.name()));
                }
                info
            }

            "/cron" => {
                "定时任务由调度器管理，当前 session 不能直接查看。\n如需注册，走调度器 API 呢~".into()
            }

            "/models" => {
                let models = self.providers.list();
                if models.is_empty() {
                    "没有注册任何 provider 呢".into()
                } else {
                    let current = self.providers.get_default().map(|p| p.name().to_string());
                    let mut out = format!("已注册 provider（{}）：", models.len());
                    for m in &models {
                        let tag = if Some(m.as_str()) == current.as_deref() { " ⬅️ 当前" } else { "" };
                        out.push_str(&format!("\n  {}{}", m, tag));
                    }
                    out
                }
            }

            "/model" => {
                let name = args.trim();
                if name.is_empty() {
                    // 无参数：显示当前
                    match self.providers.get_default() {
                        Some(p) => format!("当前模型：{}", p.name()),
                        None => "当前没有默认模型呢~".into(),
                    }
                } else {
                    // 切换
                    match self.providers.set_default(name) {
                        Ok(()) => format!("已切换到「{}」，重启后会恢复默认喔~", name),
                        Err(_) => format!("没有叫「{}」的 provider 呢。看看 /models 有哪些呢~", name),
                    }
                }
            }

            "/quit" | "/exit" | "/q" => "再见~".into(),

            _ => format!("不认识「{}」。试试 /help 呢~", cmd),
        }
    }
}

impl SessionScheduler {
    pub fn new(
        modules: ModuleRegistry,
        providers: Arc<ProviderRegistry>,
        executor: Arc<dyn ToolExecutor + Send + Sync>,
        base_soul: &str,
    ) -> (Self, mpsc::Receiver<OutboundMessage>) {
        let (inbound_tx, inbound_rx) = mpsc::channel();
        let (outbound_tx, outbound_rx) = mpsc::channel();

        let pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let idle_timeout = Duration::from_secs(300);
        let (death_tx, worker_deaths) = mpsc::channel();

        (
            Self {
                workers: HashMap::new(),
                worker_channels: HashMap::new(),
                inbound_tx,
                inbound_rx,
                outbound_tx,
                pending_results: pending_results.clone(),
                shared: Arc::new(SchedulerShared {
                    modules,
                    providers,
                    executor,
                    base_soul: base_soul.to_string(),
                }),
                idle_timeout,
                death_tx,
                worker_deaths,
            },
            outbound_rx,
        )
    }

    /// 获取入站发送端——外部通过它投递消息
    pub fn inbound(&self) -> mpsc::Sender<SessionTask> {
        self.inbound_tx.clone()
    }

    /// 注册一个待返回结果——外部等待子 session 完成时使用
    pub fn register_pending(&self) -> (String, mpsc::Receiver<String>) {
        let id = format!("pending-{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos());
        let (tx, rx) = mpsc::channel();
        if let Ok(mut map) = self.pending_results.lock() {
            map.insert(id.clone(), tx);
        }
        (id, rx)
    }

    /// 获取 pending_results 的 Arc 引用（供外部注入 DelegationModule）
    pub fn pending_results_arc(&self) -> Arc<Mutex<HashMap<String, mpsc::Sender<String>>>> {
        self.pending_results.clone()
    }

    /// 投递一条消息——调度器自动路由到对应 worker
    pub fn dispatch(&mut self, task: SessionTask) {
        let sid = task.session_id.clone();
        // 克隆一份消息——已有的 worker 可能吃掉它
        let task_for_new = SessionTask {
            session_id: sid.clone(),
            input: task.input.clone(),
            channel: task.channel.clone(),
            sender: task.sender.clone(),
        };

        // 如果已有 worker，先尝试发送——失败说明 worker 已退出，清理后重建
        if let Some(tx) = self.worker_channels.get(&sid) {
            if tx.send(task).is_ok() {
                return;
            }
            tracing::info!("scheduler: cleaning stale worker for session '{}'", sid);
            self.workers.remove(&sid);
            self.worker_channels.remove(&sid);
        }

        // 创建新的 worker
        let (worker_tx, worker_rx) = mpsc::channel::<SessionTask>();

        let worker = SessionWorker::new(
            sid.clone(),
            &self.shared.base_soul,
            self.shared.modules.clone(),
            self.shared.providers.clone(),
            self.shared.executor.clone(),
            self.outbound_tx.clone(),
            worker_rx,
            self.pending_results.clone(),
            self.idle_timeout,
            self.death_tx.clone(),
        );

        let handle = thread::Builder::new()
            .name(format!("session-{}", sid))
            .spawn(move || worker.run())
            .expect("failed to spawn session worker");

        // 存储 worker 发送端和线程句柄
        self.worker_channels.insert(sid.clone(), worker_tx.clone());
        self.workers.insert(sid.clone(), SessionWorkerHandle {
            session_id: sid.clone(),
            _thread: handle,
        });

        // 把触发的消息发给新 worker
        if let Err(e) = worker_tx.send(task_for_new) {
            tracing::error!(
                "scheduler: failed to forward task to new worker for session '{}': {}",
                sid, e
            );
        }
    }

    /// 运行调度器主循环——阻塞等待入站消息，同时监控 worker 死亡通知
    pub fn run(&mut self) {
        tracing::info!("scheduler: started");

        loop {
            // 扫一遍死者名单——panic 的 worker 在此清理
            while let Ok(dead_sid) = self.worker_deaths.try_recv() {
                tracing::warn!("scheduler: cleaning up panicked worker '{}'", dead_sid);
                self.workers.remove(&dead_sid);
                self.worker_channels.remove(&dead_sid);
            }

            match self.inbound_rx.recv() {
                Ok(task) => self.dispatch(task),
                Err(_) => break,
            }
        }

        tracing::info!("scheduler: stopped");
    }
}

impl Drop for SessionScheduler {
    fn drop(&mut self) {
        tracing::info!("scheduler: shutting down {} workers", self.workers.len());

        // 关闭所有 worker 的入站通道——worker 收到 Disconnected 后自动退出
        self.worker_channels.clear();

        // 收集所有线程句柄，逐个等待完成
        let handles: Vec<thread::JoinHandle<()>> = self.workers.drain()
            .map(|(_, handle)| handle._thread)
            .collect();

        for handle in handles {
            let _ = handle.join();
        }

        tracing::info!("scheduler: all workers stopped");
    }
}
