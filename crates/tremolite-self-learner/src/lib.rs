/// 自我学习器——钻文档未覆盖的缺口，用 LLM 生成新技能定义
///
/// 实现 Module trait，声明依赖 skill 模块。
/// 如果未注册 skill 模块，引擎会拒绝注册并报错。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use tremolite_core::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError, ToolDefinition,
};

/// LLM 调用回调
type LlmFn = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// 自我学习器模块
pub struct SelfLearnerModule {
    llm: LlmFn,
    last_learn_time: u64,
    learn_interval: u64,
}

impl SelfLearnerModule {
    pub fn new(llm: LlmFn) -> Self {
        Self {
            llm,
            last_learn_time: 0,
            learn_interval: 7200,
        }
    }
}

impl Module for SelfLearnerModule {
    fn id(&self) -> &str { "self-learner" }
    fn name(&self) -> &str { "自我学习器" }
    fn version(&self) -> &str { "0.1.0" }

    fn provides(&self) -> Vec<Capability> { vec!["skill.self_learn".into()] }
    fn requires(&self) -> Vec<Capability> { vec![] }

    fn required_modules(&self) -> Vec<&str> { vec!["skill"] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> { vec![] }

    fn execute_tool(&mut self, _name: &str, _args: &str) -> Result<String, ModuleError> {
        Err(ModuleError::ToolNotFound("no tools".into()))
    }

    fn on_event(&mut self, _event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        Ok(EventResponse::Pass)
    }

    fn as_any(&self) -> Option<&dyn Any> { None }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { None }
}
