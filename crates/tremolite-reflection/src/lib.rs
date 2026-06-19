use std::sync::atomic::{AtomicU64, Ordering};

use tremolite_core::module::{
    Capability, EngineHandle, Event, EventContext, EventResponse, Module, ModuleError,
};
use tremolite_core::MemoryModule;
use tremolite_llm::{Message, ProviderRegistry};

// ─── 辩证法 key 约定（与 prompt 注入器共享） ────────

/// L2 中存储 peer card 的 key
pub const KEY_PEER_CARD: &str = "user_peer";
/// L2 中存储辩证法结论的 key 前缀（后接时间戳）
pub const KEY_DIALECTIC_PREFIX: &str = "dialectic";

// ─── ReflectionEngine ──────────────────────────────

pub struct ReflectionEngine {
    trigger_interval: u64,
    message_count: AtomicU64,
    last_reflection_seq: AtomicU64,
}

impl ReflectionEngine {
    pub fn new(trigger_interval: u64) -> Self {
        Self {
            trigger_interval,
            message_count: AtomicU64::new(0),
            last_reflection_seq: AtomicU64::new(0),
        }
    }

    pub fn message_count(&self) -> u64 {
        self.message_count.load(Ordering::Relaxed)
    }

    pub fn count_and_check(&self) -> bool {
        let count = self.message_count.fetch_add(1, Ordering::Relaxed) + 1;
        let last = self.last_reflection_seq.load(Ordering::Relaxed);
        if count - last >= self.trigger_interval {
            self.last_reflection_seq.store(count, Ordering::Relaxed);
            true
        } else {
            false
        }
    }

    /// 执行辩证法推理：读 L1 最近对话 + 当前 peer card，
    /// 通过 LLM 分析用户模式/偏好/状态，写结论到 L2
    /// 返回分析文本（供 prompt_segment 缓存）
    fn run_dialectic(engine: &EngineHandle) -> Option<String> {
        // 通过 EngineHandle 访问 MemoryModule
        let result: Option<Option<String>> = engine.with_module("memory", |m| {
            let mm = match m.as_any_mut().and_then(|a| a.downcast_mut::<MemoryModule>()) {
                Some(mm) => mm,
                None => {
                    tracing::warn!("reflection: memory module not found for dialectic");
                    return None;
                }
            };

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();

            // 1. 读 L1 最近 20 条对话
            let recent = mm.recent_entries("reflection", 20);
            let conversation: Vec<String> = recent.iter().filter_map(|e| {
                let c = &e.content;
                if c.starts_with("kamisama: ") || c.starts_with("葵: ") {
                    Some(c.clone())
                } else {
                    None
                }
            }).collect();

            if conversation.is_empty() {
                tracing::debug!("reflection: no recent conversation for dialectic");
                return None;
            }

            // 2. 读当前 peer card
            let current_card = mm.manager_mut().l2.get(KEY_PEER_CARD)
                .map(|e| e.content.clone())
                .unwrap_or_default();

            // 3. 通过 LLM 分析对话
            let analysis = Self::analyze_with_llm(engine, &conversation, &current_card);

            let content = match &analysis {
                Some(text) => {
                    format!(
                        "[dialectic:{}]\nconversation_entries: {}\nprevious_card: {}\nanalysis:\n{}",
                        now,
                        conversation.len(),
                        &current_card.chars().take(80).collect::<String>(),
                        text,
                    )
                }
                None => {
                    // LLM 不可用，退化到简单摘要
                    format!(
                        "[dialectic:{}]\nconversation_entries: {}\n(llm unavailable — degraded)",
                        now,
                        conversation.len(),
                    )
                }
            };

            // 4. 写入 L2（只打 dialectic + profile 标签，反思走正常代谢降级）
            let key = format!("{}:{}", KEY_DIALECTIC_PREFIX, now);
            mm.manager_mut().l2.set(
                &key,
                content,
                vec!["dialectic".into(), "profile".into()],
                0.6,
            );

            tracing::info!("reflection: wrote dialectic to L2 key='{}'", key);

            // 5. 如有 LLM 分析结果，写入 ProfileCache
            if let Some(analysis_text) = &analysis {
                let mut traits = std::collections::HashMap::new();
                traits.insert("dialectic_summary".into(), analysis_text.chars().take(200).collect());
                mm.update_profile(
                    "default",
                    String::new(),
                    traits,
                    None,
                    vec!["dialectic".into(), "profile".into()],
                    None,
                );
            }

            // 返回 LLM 分析文本（供 prompt_segment 缓存）
            analysis
        });

        match result {
            Some(Some(text)) => {
                tracing::debug!("reflection: dialectic produced analysis ({} chars)", text.len());
                Some(text)
            }
            Some(None) => {
                tracing::debug!("reflection: dialectic ran but no LLM analysis available");
                None
            }
            None => {
                tracing::warn!("reflection: failed to access memory module for dialectic");
                None
            }
        }
    }

    /// 通过 LLM 分析对话，返回分析文本
    fn analyze_with_llm(
        engine: &EngineHandle,
        conversation: &[String],
        card: &str,
    ) -> Option<String> {
        let providers: std::sync::Arc<ProviderRegistry> = engine.get_providers()?;
        let provider = providers.get_default()?;

        // 构建分析 prompt
        let dialogue = conversation.join("\n");
        let system_prompt = format!(
            "你是一个对话分析引擎。你的任务是分析一段对话，找出用户的：
1. 当前情绪状态（焦虑、平静、开心、疲惫等）
2. 主要话题（工作、生活、技术、游戏等）
3. 行为模式（主动发问、倾诉、抱怨、逃避等）
4. 对你的态度变化（亲近度、信任度）

已知的用户画像：
{}

对话记录：
{}

请用中文输出分析结论，保持简洁，每条一到两行。不要评价自己的分析质量。",
            if card.is_empty() { "（暂无画像数据）" } else { card },
            dialogue,
        );

        let messages = vec![
            Message::system(&system_prompt),
            Message::user("请分析以上对话"),
        ];

        match provider.chat(&messages, &[]) {
            Ok(response) => {
                tracing::info!(
                    "reflection: LLM dialectic complete ({} tokens used)",
                    response.usage.as_ref().map(|u| u.total_tokens).unwrap_or(0)
                );
                Some(response.content)
            }
            Err(e) => {
                tracing::warn!("reflection: LLM dialectic failed: {}", e);
                None
            }
        }
    }
}

// ─── ReflectionModule ──────────────────────────────

/// 反思模块
pub struct ReflectionModule {
    engine: ReflectionEngine,
    handle: Option<EngineHandle>,
    /// 最近一次辩证法分析结果缓存（同步方式更新，供 prompt_segment 注入本轮）
    latest_dialectic: String,
}

impl ReflectionModule {
    pub fn new(trigger_interval: u64) -> Self {
        Self {
            engine: ReflectionEngine::new(trigger_interval),
            handle: None,
            latest_dialectic: String::new(),
        }
    }
}

impl Module for ReflectionModule {
    fn id(&self) -> &str {
        "reflection"
    }

    fn name(&self) -> &str {
        "反思引擎"
    }

    fn version(&self) -> &str {
        "0.3.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "reflection.trigger".into(),
            "reflection.profile".into(),
            "reflection.dialectic".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        Vec::new()
    }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<tremolite_core::module::ToolDefinition> {
        vec![]
    }

    fn prompt_segment(&self) -> Option<String> {
        if self.latest_dialectic.is_empty() {
            None
        } else {
            Some(format!(
                "[反射分析]\n对话模式提示：\n{}\n---",
                self.latest_dialectic
            ))
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
                tracing::info!(
                    "reflection: engine started, trigger every {} messages",
                    self.engine.trigger_interval
                );
                Ok(EventResponse::Pass)
            }
            Event::OnMessage { .. } => {
                if self.engine.count_and_check() {
                    tracing::info!(
                        "reflection: triggered at message #{}, scheduling dialectic",
                        self.engine.message_count()
                    );

                    // 同步运行辩证法——在当前线程完成后再走 prompt_segment，
                    // 确保分析结果能注入到本轮对话而非下一轮。
                    if let Some(ref handle) = self.handle {
                        let h = handle.clone();
                        if let Some(analysis) = ReflectionEngine::run_dialectic(&h) {
                            self.latest_dialectic = analysis;
                            tracing::info!("reflection: dialectic complete, injected to current turn");
                        } else {
                            tracing::warn!("reflection: dialectic returned no analysis");
                        }
                    }

                    Ok(EventResponse::Pass)
                } else {
                    Ok(EventResponse::Pass)
                }
            }
            Event::Shutdown => {
                tracing::info!(
                    "reflection: shutting down, processed {} messages",
                    self.engine.message_count()
                );
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn display_status(&self) -> Option<String> {
        Some(format!(
            "反思: msg#{}",
            self.engine.message_count()
        ))
    }
}
