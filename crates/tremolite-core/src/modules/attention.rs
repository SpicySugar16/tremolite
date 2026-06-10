use std::any::Any;
use tremolite_attention::{MultiScaleAttention, AttentionScale, AttentionResult};
use tremolite_llm::ToolDefinition;
use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// 注意力模块——多尺度滑动窗口扫描
pub struct AttentionModule {
    engine: MultiScaleAttention,
    last_summary: String,
}

impl AttentionModule {
    pub fn new() -> Self {
        Self {
            engine: MultiScaleAttention::new(),
            last_summary: String::new(),
        }
    }

    pub fn with_embedding_api(mut self, base: &str, key: &str, model: &str) -> Self {
        self.engine = std::mem::take(&mut self.engine)
            .with_embedding_api(base, key, model);
        self
    }

    pub fn summary(&self) -> &str { &self.last_summary }
    pub fn engine(&self) -> &MultiScaleAttention { &self.engine }
    pub fn last_result(&self) -> Option<&AttentionResult> { self.engine.last_result() }
}

impl Module for AttentionModule {
    fn id(&self) -> &str { "attention" }
    fn name(&self) -> &str { "多尺度注意力" }
    fn version(&self) -> &str { "0.2.0" }

    fn provides(&self) -> Vec<Capability> {
        vec!["attention.scan".into(), "attention.synthesis".into()]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> { vec![] }

    fn prompt_segment(&self) -> Option<String> {
        if self.last_summary.is_empty() {
            None
        } else {
            Some(format!(
                "[注意力扫描结果]\n对话中的关键内容：{}\n（注意力引擎：多尺度语义模型）",
                self.last_summary
            ))
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        // 注意：attend() 同步调外部 embedding API 会阻塞模块注册表锁，
        // 导致其他 worker 无法访问任何模块（含 MemoryModule）。
        // 注意力扫描后续通过独立的方法或定时任务触发。
        // 当前 on_event 不做任何阻塞操作。
        if let Event::OnMessage { .. } = event {
            // 留空——注意力扫描由外部工具或 cron 触发
        }
        Ok(EventResponse::Pass)
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
}
