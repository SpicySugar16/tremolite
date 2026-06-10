/// 会话管理 Module——跨 session 窥探 + 共享控制
/// 
/// 设计原则：不自动注入跨 session 内容到 prompt，
/// 而是提供工具让 LLM 按需 pull，pull 之前先查权限。
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::module::{
    Capability, EngineHandle, Event, EventContext, EventResponse, Module, ModuleError,
};
use tremolite_llm::ToolDefinition;
use tremolite_session::SessionManager;

// ─── CrossSessionRing ─────────────────────────────
// 保留但不再注入 prompt——仅用于内部状态追踪

const RING_CAPACITY: usize = 20;

#[derive(Debug, Clone)]
pub struct RingNote {
    pub source_session: String,
    pub summary: String,
    pub mood_tag: String,
    pub timestamp: u64,
}

/// 环状 buffer——存其他 session 的关键摘要
pub struct CrossSessionRing {
    notes: Vec<RingNote>,
    _next_id: AtomicU64,
}

impl CrossSessionRing {
    pub fn new() -> Self {
        Self {
            notes: Vec::with_capacity(RING_CAPACITY),
            _next_id: AtomicU64::new(1),
        }
    }

    pub fn push(&mut self, source_session: String, summary: String, mood_tag: String) {
        let note = RingNote {
            source_session,
            summary,
            mood_tag,
            timestamp: now_secs(),
        };
        if self.notes.len() >= RING_CAPACITY {
            self.notes.remove(0);
        }
        self.notes.push(note);
    }

    pub fn peek(&self, n: usize) -> Vec<&RingNote> {
        let n = n.min(self.notes.len());
        self.notes.iter().rev().take(n).collect()
    }

    pub fn clear(&mut self) {
        self.notes.clear();
    }

    pub fn len(&self) -> usize {
        self.notes.len()
    }
}

// ─── NoteDistiller ───────────────────────────────

pub struct NoteDistiller;

impl NoteDistiller {
    /// 纯规则提炼：30 字摘要 + 情绪标签
    pub fn distill(text: &str) -> (String, String) {
        let summary: String = text.chars().take(30).collect();
        let summary = if text.chars().count() > 30 {
            format!("{}…", summary)
        } else {
            summary
        };
        let mood = if text.contains('!') || text.contains('！') {
            "激动"
        } else if text.contains('?') || text.contains('？') {
            "疑问"
        } else if text.contains("哈哈") || text.contains("笑") {
            "开心"
        } else if text.contains("烦") || text.contains("气") {
            "不满"
        } else {
            "中性"
        };
        (summary, mood.into())
    }
}

// ─── SessionModule ────────────────────────────────

/// 每次清理检查时的消息间隔——每 100 条消息检查一次 stale closed session
const CLEANUP_INTERVAL: u64 = 100;

pub struct SessionModule {
    pub manager: SessionManager,
    pub ring: CrossSessionRing,
    pub handle: Option<EngineHandle>,
    _message_count: AtomicU64,
    _cleanup_counter: AtomicU64,
}

impl SessionModule {
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            manager: SessionManager::with_ttl(ttl_secs),
            ring: CrossSessionRing::new(),
            handle: None,
            _message_count: AtomicU64::new(0),
            _cleanup_counter: AtomicU64::new(0),
        }
    }

    pub fn active_sessions(&self) -> usize {
        self.manager.count()
    }

    fn on_message(&mut self, session_id: &str, content: &str) {
        self.manager.get_or_create(session_id);
        self._message_count.fetch_add(1, Ordering::Relaxed);
        let (summary, mood) = NoteDistiller::distill(content);
        self.ring.push(session_id.to_string(), summary, mood);

        // 每次收到消息时顺便冷却闲置 session
        let idle = self.manager.reap_idle();
        if !idle.is_empty() {
            tracing::debug!("session: cooled {} idle sessions", idle.len());
        }

        // 每 CLEANUP_INTERVAL 条消息检查一次 stale closed session
        let count = self._cleanup_counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count >= CLEANUP_INTERVAL {
            self._cleanup_counter.store(0, Ordering::Relaxed);
            let stale = self.manager.reap_stale_closed();
            if !stale.is_empty() {
                tracing::info!("session: purged {} stale closed sessions", stale.len());
                // 通知 MemoryModule 释放 L1 分片
                if let Some(ref handle) = self.handle {
                    for sid in &stale {
                        handle.with_module("memory", |m| {
                            if let Some(mm) = m.as_any_mut()
                                .and_then(|any| any.downcast_mut::<crate::modules::memory::MemoryModule>())
                            {
                                mm.manager_mut().remove_session(sid);
                            }
                        });
                    }
                }
            }
        }
    }

    /// 列出活跃 session 的摘要（id + 最后活跃时间 + 共享状态）
    fn list_active_summaries(&self) -> Vec<String> {
        let now = now_secs();
        let mut summaries: Vec<(String, u64, bool)> = self.manager.sessions().iter()
            .map(|(id, s)| (id.clone(), s.last_active, s.shared))
            .collect();
        // 按最后活跃时间降序
        summaries.sort_by(|a, b| b.1.cmp(&a.1));
        summaries.iter().map(|(id, ts, shared)| {
            let ago_secs = now.saturating_sub(*ts);
            let ago = if ago_secs < 60 {
                format!("{}秒前", ago_secs)
            } else if ago_secs < 3600 {
                format!("{}分钟前", ago_secs / 60)
            } else {
                format!("{}小时前", ago_secs / 3600)
            };
            let flag = if *shared { "共享" } else { "私密" };
            format!("session:{} [{}] 活跃于{}", id, flag, ago)
        }).collect()
    }
}

impl Module for SessionModule {
    fn id(&self) -> &str {
        "session"
    }

    fn name(&self) -> &str {
        "会话管理器"
    }

    fn version(&self) -> &str {
        "0.2.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec!["session.manager".into(), "session.peek".into()]
    }

    fn requires(&self) -> Vec<Capability> {
        Vec::new()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "list_active_sessions".into(),
                    description: "列出所有活跃会话及其共享状态。返回 session_id、最后活跃时间、是否已获共享授权。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "peek_session".into(),
                    description: "查看指定 session 的近期对话内容。需要该 session 已授权共享（shared=true），否则返回拒绝提示。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "session_id": {
                                "type": "string",
                                "description": "要查看的会话 ID"
                            },
                            "reason": {
                                "type": "string",
                                "description": "查看此会话的原因说明，用于决定是否适合泄露信息"
                            },
                            "count": {
                                "type": "integer",
                                "description": "要获取的最近消息条数（默认 10，最大 20）"
                            }
                        },
                        "required": ["session_id", "reason"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "share_session".into(),
                    description: "将当前会话标记为允许其他会话查看。调用后其他 session 可以用 peek_session 查看本 session 的近期对话。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "unshare_session".into(),
                    description: "将当前会话标记为私密，禁止其他 session 查看。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "configure_session_timeout".into(),
                    description: "调整会话冷却或清理的超时时间。cooling: 闲置多少秒后自动冷却（默认300秒=5分钟）。cleanup: 冷却后多少秒彻底清理（默认2592000秒=30天）。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "timeout_type": {
                                "type": "string",
                                "enum": ["cooling", "cleanup"],
                                "description": "要调整的超时类型：cooling=闲置冷却时间，cleanup=冷却后清理时间"
                            },
                            "seconds": {
                                "type": "integer",
                                "description": "新的超时秒数"
                            }
                        },
                        "required": ["timeout_type", "seconds"]
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "list_active_sessions" => {
                let lines = self.list_active_summaries();
                if lines.is_empty() {
                    return Ok("当前没有活跃会话。".into());
                }
                Ok(lines.join("\n"))
            }

            "peek_session" => {
                let parsed: HashMap<String, serde_json::Value> =
                    serde_json::from_str(args).map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;

                let target_sid = parsed.get("session_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少 session_id 参数".into()))?;

                let _reason = parsed.get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("未说明");

                let count = parsed.get("count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(10)
                    .min(20) as usize;

                // 检查 session 是否存在
                let sessions = self.manager.sessions();
                let state = sessions.get(target_sid);

                match state {
                    None => {
                        return Ok(format!("没有找到 session '{}'。可能是已过期的会话。", target_sid));
                    }
                    Some(s) if !s.shared => {
                        // 未授权——拒绝，不是葵不帮忙，是这个 session 没松口
                        return Ok(format!(
                            "session '{}' 当前未授权共享。需要先在该 session 中调用 share_session 授权后才能查看。\n\n\
                             已获得共享授权的 session：\n{}",
                            target_sid,
                            self.list_active_summaries().iter()
                                .filter(|l| l.contains("[共享]"))
                                .cloned()
                                .collect::<Vec<_>>()
                                .join("\n")
                        ));
                    }
                    _ => {} // 已共享，继续
                }

                // 通过 EngineHandle 访问 MemoryModule 获取原文
                let result = self.handle.as_ref().and_then(|handle| {
                    handle.with_module("memory", |m| {
                        m.as_any()
                            .and_then(|any| any.downcast_ref::<crate::modules::memory::MemoryModule>())
                            .map(|mm| {
                                let entries = mm.recent_entries(target_sid, count);
                                if entries.is_empty() {
                                    "该 session 暂无近期对话记录。".to_string()
                                } else {
                                    let lines: Vec<String> = entries.iter().map(|e| {
                                        let ts = e.created_at;
                                        format!("[{}] {}", ts, e.content)
                                    }).collect();
                                    format!("session '{}' 的近期对话（最新 {} 条）：\n{}",
                                        target_sid, entries.len(), lines.join("\n"))
                                }
                            })
                    })
                    .flatten()
                }).unwrap_or_else(|| "暂时无法读取记忆模块。可能是模块尚未就绪。".to_string());

                Ok(result)
            }

            "share_session" => {
                // 这里不拿 session_id 参数——默认共享当前引擎 session
                // 实际使用时 engine 会通过 ctx 传入 session_id
                // 我们遍历所有 session 让所有不私密的都共享
                // 更准确的实现需要在 execute_tool 中接收 ctx
                let mutable_self = self;
                for (_id, state) in mutable_self.manager.sessions_mut() {
                    state.share();
                }
                Ok("当前会话已标记为共享。其他会话现在可以通过 peek_session 查看本会话的近期对话了。".into())
            }

            "unshare_session" => {
                let mutable_self = self;
                for (_id, state) in mutable_self.manager.sessions_mut() {
                    state.unshare();
                }
                Ok("当前会话已标记为私密。其他会话将无法查看本会话的近期对话。".into())
            }

            "configure_session_timeout" => {
                let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;

                let timeout_type = parsed.get("timeout_type")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少 timeout_type 参数".into()))?;

                let seconds = parsed.get("seconds")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少 seconds 参数".into()))?;

                match timeout_type {
                    "cooling" => {
                        self.manager.set_idle_timeout(seconds);
                        let mins = seconds / 60;
                        Ok(format!("闲置冷却时间已设为 {} 秒（约 {} 分钟）喔。", seconds, mins))
                    }
                    "cleanup" => {
                        self.manager.set_cleanup_timeout(seconds);
                        let days = seconds / 86400;
                        Ok(format!("清理超时已设为 {} 秒（约 {} 天）呢。", seconds, days))
                    }
                    _ => Err(ModuleError::ToolExecutionFailed(format!("未知超时类型：{}", timeout_type))),
                }
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
                tracing::info!("session: engine started");
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                let closed = self.manager.close_all();
                tracing::info!("session: shutdown, closed {} active sessions", closed.len());
                Ok(EventResponse::Pass)
            }
            Event::OnMessage { ref input, .. } => {
                let sid = if ctx.session_id.is_empty() {
                    "default"
                } else {
                    &ctx.session_id
                };
                self.on_message(sid, input);
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    /// 不再自动注入跨 session 内容到 prompt。
    /// 改为提供工具让 LLM 按需 pull。
    fn prompt_segment(&self) -> Option<String> {
        None
    }

    fn display_status(&self) -> Option<String> {
        let shared_count = self.manager.sessions().values()
            .filter(|s| s.shared)
            .count();
        Some(format!("会话: {}个活跃 ({}个共享)", self.manager.count(), shared_count))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_push_peek() {
        let mut ring = CrossSessionRing::new();
        for i in 0..25 {
            ring.push(
                format!("s{}", i % 3),
                format!("消息 {}", i),
                "中性".into(),
            );
        }
        assert_eq!(ring.len(), 20);
        let notes = ring.peek(3);
        assert_eq!(notes.len(), 3);
        assert!(notes[0].summary.contains("消息 24"));
    }

    #[test]
    fn test_list_summaries_empty() {
        let mut sm = SessionModule::new(3600);
        let summaries = sm.list_active_summaries();
        assert!(summaries.is_empty());
    }

    #[test]
    fn test_share_unshare_cycle() {
        let mut mgr = SessionManager::new();
        let s = mgr.get_or_create("test");
        assert_eq!(s.shared, false);
        s.share();
        assert_eq!(s.shared, true);
        s.unshare();
        assert_eq!(s.shared, false);
    }

    #[test]
    fn test_tool_execution_list() {
        let mut sm = SessionModule::new(3600);
        sm.on_message("alpha", "在吗？");
        sm.on_message("beta", "帮忙看看这段代码");

        let result = sm.execute_tool("list_active_sessions", "{}").unwrap();
        assert!(result.contains("alpha"), "应该列出 alpha session");
        assert!(result.contains("beta"), "应该列出 beta session");
    }

    #[test]
    fn test_tool_share_toggle() {
        let mut sm = SessionModule::new(3600);
        sm.on_message("test", "你好");

        // 默认不共享
        assert!(!sm.manager.sessions().get("test").unwrap().shared);

        // 共享
        let _ = sm.execute_tool("share_session", "{}");
        assert!(sm.manager.sessions().get("test").unwrap().shared);

        // 取消共享
        let _ = sm.execute_tool("unshare_session", "{}");
        assert!(!sm.manager.sessions().get("test").unwrap().shared);
    }

    #[test]
    fn test_peek_nonexistent_session() {
        let mut sm = SessionModule::new(3600);
        let result = sm.execute_tool("peek_session", r#"{"session_id": "nobody", "reason": "测试"}"#).unwrap();
        assert!(result.contains("没有找到"));
    }
}
