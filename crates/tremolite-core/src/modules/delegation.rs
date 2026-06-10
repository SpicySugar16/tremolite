use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tremolite_delegation::{DelegationEngine, DelegateMode, TaskContext, TaskHandle, TaskStatus};
use tremolite_llm::{ToolDefinition, ToolFunction};
use crate::module::{
    Capability, EngineHandle, Event, EventContext, EventResponse, Module, ModuleError,
};
use crate::modules::session::SessionModule;
use crate::modules::memory::MemoryModule;
use crate::scheduler::SessionTask;

/// 委派模块——让透闪石能 spawn 子进程/外部工具做任务
///
/// 支持 tool 委派和 session 委派两种模式：
/// - `delegate_task` — spawn 子进程做一次性任务
/// - `delegate_session` — 创建子会话，记录上下文，结果可跨 session 查看
pub struct DelegationModule {
    active_tasks: HashMap<String, TaskHandle>,
    completed_tasks: HashMap<String, String>,
    task_seq: u64,
    default_timeout: u64,
    handle: Option<EngineHandle>,
    session_seq: u64,
    /// 调度器入站通道——子 session 消息通过它投递
    scheduler_tx: Option<mpsc::Sender<SessionTask>>,
    /// 待返回结果映射——子 session 完成时通知父 session
    pending_results: Option<Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>>,
}

impl DelegationModule {
    pub fn new() -> Self {
        Self {
            active_tasks: HashMap::new(),
            completed_tasks: HashMap::new(),
            task_seq: 0,
            default_timeout: 300,
            handle: None,
            session_seq: 0,
            scheduler_tx: None,
            pending_results: None,
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.default_timeout = secs;
        self
    }

    /// 设置调度器入站通道（由 Engine 在创建调度器后调用）
    pub fn set_scheduler(&mut self, tx: mpsc::Sender<SessionTask>, pending: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>) {
        self.scheduler_tx = Some(tx);
        self.pending_results = Some(pending);
    }

    fn next_task_id(&mut self) -> String {
        self.task_seq += 1;
        format!("task-{}", self.task_seq)
    }

    fn next_session_id(&mut self, parent_id: &str) -> String {
        self.session_seq += 1;
        format!("{}:delegate-{}", parent_id, self.session_seq)
    }

    fn delegate(&mut self, mode: DelegateMode, ctx: TaskContext) -> Result<String, String> {
        let id = self.next_task_id();
        let timeout = Duration::from_secs(ctx.timeout_secs.max(10));
        let result = DelegationEngine::spawn_and_wait(mode, ctx, timeout)?;
        self.completed_tasks.insert(id.clone(), result.clone());
        Ok(format!("[task:{}] 完成: {}", id, result))
    }

    fn cleanup_completed(&mut self) {
        let mut done_ids = Vec::new();
        for (id, handle) in &self.active_tasks {
            if handle.is_done() {
                if let TaskStatus::Completed(result) = &handle.status {
                    self.completed_tasks.insert(id.clone(), result.clone());
                }
                done_ids.push(id.clone());
            }
        }
        for id in done_ids {
            let _ = self.active_tasks.remove(&id);
        }
    }

    fn list_completed_summary(&self) -> Vec<String> {
        self.completed_tasks
            .iter()
            .map(|(id, result)| {
                let preview: String = result.chars().take(80).collect();
                format!("[{}] {}...", id, preview)
            })
            .collect()
    }
}

impl Module for DelegationModule {
    fn id(&self) -> &str { "delegation" }
    fn name(&self) -> &str { "任务委派" }
    fn version(&self) -> &str { "0.2.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "delegation.spawn".into(),
            "delegation.list".into(),
            "delegation.session".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "delegate_task".into(),
                    description: "把任务委派给子进程或外部工具执行。支持三种模式：tremolite（子agent）、opencode（编程工具）、shell（命令）。适用于编程、搜索、批量处理等耗时任务。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "goal": { "type": "string", "description": "任务目标" },
                            "context": { "type": "string", "description": "背景信息" },
                            "mode": { "type": "string", "enum": ["tremolite", "opencode", "shell"], "description": "委派模式" },
                            "command": { "type": "string", "description": "仅 opencode 模式：CLI 名称" },
                            "workdir": { "type": "string", "description": "工作目录（可选）" },
                            "timeout_secs": { "type": "integer", "description": "超时秒数（可选，默认300）" }
                        },
                        "required": ["goal", "mode"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "delegate_session".into(),
                    description: "创建子会话执行任务。子会话有自己的 session_id 和 L1 历史记录，可被跨 session 工具（list_active_sessions、peek_session）查看。任务完成后自动结果写入子会话并冷却。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "goal": { "type": "string", "description": "委派给子会话的任务目标" },
                            "context": { "type": "string", "description": "传递给子会话的上下文（如父会话的对话摘录、配置等）" },
                            "mode": { "type": "string", "enum": ["tremolite", "opencode", "shell"], "description": "子会话执行模式" },
                            "timeout_secs": { "type": "integer", "description": "超时秒数（可选，默认300）" }
                        },
                        "required": ["goal", "mode"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "list_delegations".into(),
                    description: "查看所有已完成委派任务的结果摘要".into(),
                    parameters: serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "delegate_task" => {
                let parsed: HashMap<String, serde_json::Value> =
                    serde_json::from_str(args).unwrap_or_default();

                let goal = parsed.get("goal")
                    .and_then(|v| v.as_str()).unwrap_or("").to_string();
                if goal.is_empty() {
                    return Err(ModuleError::ToolExecutionFailed("goal 不能为空".into()));
                }

                let context = parsed.get("context")
                    .and_then(|v| v.as_str()).unwrap_or("").to_string();
                let mode_str = parsed.get("mode")
                    .and_then(|v| v.as_str()).unwrap_or("shell");
                let command = parsed.get("command")
                    .and_then(|v| v.as_str()).unwrap_or("opencode");
                let workdir = parsed.get("workdir")
                    .and_then(|v| v.as_str()).unwrap_or(".").to_string();
                let timeout = parsed.get("timeout_secs")
                    .and_then(|v| v.as_u64()).unwrap_or(self.default_timeout);

                let mode = match mode_str {
                    "tremolite" => DelegateMode::Tremolite,
                    "opencode" => DelegateMode::AcpTool {
                        command: command.to_string(),
                        args: vec!["--acp".into(), "--stdio".into()],
                    },
                    "shell" => DelegateMode::Shell { command: goal.clone() },
                    _ => DelegateMode::Shell { command: goal.clone() },
                };

                let ctx = TaskContext::new(&goal, &context)
                    .with_workdir(&workdir)
                    .with_timeout(timeout);

                match self.delegate(mode, ctx) {
                    Ok(result) => Ok(result),
                    Err(e) => Err(ModuleError::ToolExecutionFailed(e)),
                }
            }

            "delegate_session" => {
                let parsed: HashMap<String, serde_json::Value> =
                    serde_json::from_str(args).map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;

                let goal = parsed.get("goal")
                    .and_then(|v| v.as_str()).ok_or_else(|| ModuleError::ToolExecutionFailed("goal 不能为空".into()))?;
                let context = parsed.get("context")
                    .and_then(|v| v.as_str()).unwrap_or("");
                let mode_str = parsed.get("mode")
                    .and_then(|v| v.as_str()).unwrap_or("shell");
                let timeout = parsed.get("timeout_secs")
                    .and_then(|v| v.as_u64()).unwrap_or(self.default_timeout);

                let handle = self.handle.clone().ok_or_else(|| {
                    ModuleError::ToolExecutionFailed("engine 尚未就绪，无法创建子会话".into())
                })?;

                // 生成子 session ID（父 session 无可用 ID 时用 "root"）
                let parent_sid = {
                    let mut sid = String::new();
                    handle.with_module("session", |m| {
                        if let Some(sm) = m.as_any().and_then(|a| a.downcast_ref::<SessionModule>()) {
                            // 取第一个活跃 session 的 id 作为父 ID（简化处理）
                            if let Some(id) = sm.manager.sessions().keys().next() {
                                sid = id.clone();
                            }
                        }
                    });
                    if sid.is_empty() { "root".to_string() } else { sid }
                };

                let sub_sid = self.next_session_id(&parent_sid);

                // 在 SessionModule 中注册子会话
                handle.with_module("session", |m| {
                    if let Some(sm) = m.as_any_mut().and_then(|a| a.downcast_mut::<SessionModule>()) {
                        sm.manager.get_or_create(&sub_sid);
                        // 自动共享，让父 session 能 peek
                        if let Some(state) = sm.manager.sessions_mut().get_mut(&sub_sid) {
                            state.share();
                        }
                    }
                });

                // 在 MemoryModule 中写入子会话的初始上下文
                handle.with_module("memory", |m| {
                    if let Some(mm) = m.as_any_mut().and_then(|a| a.downcast_mut::<MemoryModule>()) {
                        mm.manager_mut().remember(
                            &sub_sid,
                            format!("[delegate_start] 目标: {}", goal),
                            vec!["delegate".into(), "session_start".into()],
                            0.6, "delegation".into(),
                        );
                        if !context.is_empty() {
                            mm.manager_mut().remember(
                                &sub_sid,
                                format!("[delegate_context] {}", context),
                                vec!["delegate".into(), "context".into()],
                                0.5, "delegation".into(),
                            );
                        }
                    }
                });

                // 执行任务
                let mode = match mode_str {
                    "tremolite" => DelegateMode::Tremolite,
                    "opencode" => DelegateMode::AcpTool {
                        command: "opencode".into(),
                        args: vec!["--acp".into(), "--stdio".into()],
                    },
                    "shell" => DelegateMode::Shell { command: goal.into() },
                    _ => DelegateMode::Shell { command: goal.into() },
                };

                let ctx = TaskContext::new(goal, context).with_timeout(timeout);

                tracing::info!("delegation: delegate_session sid={} mode={mode_str} goal={}", &sub_sid, &goal[..goal.len().min(60)]);

                // mode: "tremolite" 走调度器，其他模式走原有的 spawn_and_wait
                let task_result = if mode_str == "tremolite" {
                    match (&self.scheduler_tx, &self.pending_results) {
                        (Some(tx), Some(pending)) => {
                            // 在 pending_results 中注册子 session 的结果通道
                            let (result_tx, result_rx) = mpsc::channel();
                            if let Ok(mut map) = pending.lock() {
                                map.insert(sub_sid.clone(), result_tx);
                            }

                            // 投递任务到调度器
                            let _ = tx.send(SessionTask {
                                session_id: sub_sid.clone(),
                                input: format!("[delegate] 目标: {goal}\n{context}"),
                                channel: "delegation".into(),
                                sender: sub_sid.clone(),
                            });

                            // 阻塞等待结果（带超时）
                            match result_rx.recv_timeout(Duration::from_secs(timeout)) {
                                Ok(r) => r,
                                Err(_) => format!("[delegate_error] 子会话超时或通道断开"),
                            }
                        }
                        _ => {
                            // 调度器不可用，降级到 spawn_and_wait
                            match self.delegate(mode, ctx) {
                                Ok(r) => r,
                                Err(e) => format!("[delegate_error] {e}"),
                            }
                        }
                    }
                } else {
                    match self.delegate(mode, ctx) {
                        Ok(r) => r,
                        Err(e) => format!("[delegate_error] {e}"),
                    }
                };

                // 将结果写回子会话的 L1
                handle.with_module("memory", |m| {
                    if let Some(mm) = m.as_any_mut().and_then(|a| a.downcast_mut::<MemoryModule>()) {
                        mm.manager_mut().remember(
                            &sub_sid,
                            format!("[delegate_result] {}", task_result),
                            vec!["delegate".into(), "result".into()],
                            0.7, "delegation".into(),
                        );
                    }
                });

                Ok(format!(
                    "子会话 {} 已完成。\n任务目标：{}\n结果：{}\n\n可用 peek_session 查看完整上下文呢~",
                    sub_sid, goal, task_result
                ))
            }

            "list_delegations" => {
                self.cleanup_completed();
                let summaries = self.list_completed_summary();
                if summaries.is_empty() {
                    return Ok("暂无已完成委派任务~".into());
                }
                Ok(summaries.join("\n"))
            }

            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn on_event(
        &mut self,
        event: &Event,
        ctx: &EventContext,
    ) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                self.handle = Some(ctx.engine.clone());
                tracing::info!("delegation: module ready");
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                self.cleanup_completed();
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn display_status(&self) -> Option<String> {
        let total = self.completed_tasks.len();
        let active = self.active_tasks.len();
        if total > 0 || active > 0 {
            Some(format!("委派: {total}完成 {active}活跃"))
        } else {
            None
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { Some(self) }
}
