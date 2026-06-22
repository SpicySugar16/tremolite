use std::any::Any;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use tremolite_attention::{MultiScaleAttention, AttentionResult, ChatType, Channel};
use tremolite_llm::ToolDefinition;
use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// 注意力模块——链式递进多尺度扫描
pub struct AttentionModule {
    engine: MultiScaleAttention,
    last_summary: String,
    /// 注入冷却剩余轮数
    cooldown_remaining: AtomicU32,
    /// 冷却间隔（配置文件中的 inject_cooldown_rounds）
    inject_cooldown_rounds: u32,
    /// 注入日志路径——记录最后一次注入的提示文本
    inject_log_path: Option<PathBuf>,
}

impl AttentionModule {
    pub fn new() -> Self {
        Self {
            engine: MultiScaleAttention::new(),
            last_summary: String::new(),
            cooldown_remaining: AtomicU32::new(0),
            inject_cooldown_rounds: 3,
            inject_log_path: None,
        }
    }

    pub fn with_inject_log_path(mut self, path: &str) -> Self {
        self.inject_log_path = Some(PathBuf::from(path));
        self
    }

    pub fn with_embedding_api(mut self, base: &str, key: &str, model: &str) -> Self {
        self.engine = std::mem::take(&mut self.engine)
            .with_embedding_api(base, key, model);
        self
    }

    /// 配置注意力通道
    pub fn with_channels(mut self, channels: Vec<Channel>) -> Self {
        self.engine = std::mem::take(&mut self.engine)
            .with_channels(channels);
        self
    }

    /// 配置注入冷却轮数
    pub fn with_inject_cooldown(mut self, rounds: u32) -> Self {
        self.inject_cooldown_rounds = rounds;
        self
    }

    pub fn summary(&self) -> &str { &self.last_summary }
    pub fn engine(&self) -> &MultiScaleAttention { &self.engine }
    pub fn last_result(&self) -> Option<&AttentionResult> { self.engine.last_result() }
    pub fn set_stats_path(&mut self, path: &str) {
        self.engine.set_stats_path(path);
    }

    pub fn set_inject_log_path(&mut self, path: &str) {
        self.inject_log_path = Some(PathBuf::from(path));
    }
}

impl Module for AttentionModule {
    fn id(&self) -> &str { "attention" }
    fn name(&self) -> &str { "多尺度注意力" }
    fn version(&self) -> &str {
        "1.0.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec!["attention.scan".into(), "attention.synthesis".into()]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> { vec![] }

    fn prompt_segment(&self) -> Option<String> {
        let result = self.engine.last_result()?;

        // 冷却检查
        let remaining = self.cooldown_remaining.load(Ordering::Relaxed);
        if remaining > 0 {
            self.cooldown_remaining.store(remaining - 1, Ordering::Relaxed);
            return None;
        }

        // 低分链（wide 最大 score < 0.4）全程闭嘴
        let wide_max = result.channel_blocks.get("wide")
            .and_then(|b| b.first())
            .map(|b| b.score)
            .unwrap_or(0.0);
        if wide_max < 0.4 {
            return None;
        }

        // 离散碎语不注入
        if result.chat_type == ChatType::Scattered {
            return None;
        }

        let msg = if result.chain_depth >= 3 {
            "[注意力提示] 高密：被多次聚焦"
        } else if result.chat_type == ChatType::FocusDiscussion {
            "[注意力提示] 聚焦：当前话题深度高"
        } else {
            "[注意力提示] 转移：话题跨度大"
        };
        self.cooldown_remaining
            .store(self.inject_cooldown_rounds, Ordering::Relaxed);
        // 写入注入日志
        if let Some(ref log_path) = self.inject_log_path {
            let _ = std::fs::write(log_path, &msg);
        }
        Some(msg.to_string())
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        if let Event::OnMessage { input, .. } = event {
            if !input.trim().is_empty() {
                let result = self.engine.attend(input);
                self.last_summary = result.synthesis.summary.clone();
            }
        }
        Ok(EventResponse::Pass)
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { Some(self) }
}
