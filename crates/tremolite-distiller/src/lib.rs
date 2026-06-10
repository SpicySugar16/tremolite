/// 技能蒸馏器——从 practice_log 中发现高频模式，生成体系化技能
///
/// 实现 Module trait，声明依赖 skill 模块。
/// 如果未注册 skill 模块，引擎会拒绝注册并报错。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use tremolite_core::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError, ToolDefinition,
};
use tremolite_core::modules::skill::SkillModule;

/// LLM 调用回调
type LlmFn = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// 技能蒸馏器模块
pub struct DistillerModule {
    llm: LlmFn,
    last_distill_time: u64,
    distill_interval: u64,
}

impl DistillerModule {
    pub fn new(llm: LlmFn) -> Self {
        Self {
            llm,
            last_distill_time: 0,
            distill_interval: 3600,
        }
    }
}

impl Module for DistillerModule {
    fn id(&self) -> &str { "distiller" }
    fn name(&self) -> &str { "技能蒸馏器" }
    fn version(&self) -> &str { "0.1.0" }

    fn provides(&self) -> Vec<Capability> { vec!["skill.distill".into()] }
    fn requires(&self) -> Vec<Capability> { vec![] }

    fn required_modules(&self) -> Vec<&str> { vec!["skill"] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> { vec![] }

    fn execute_tool(&mut self, _name: &str, _args: &str) -> Result<String, ModuleError> {
        Err(ModuleError::ToolNotFound("no tools".into()))
    }

    fn on_event(&mut self, event: &Event, ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                tracing::info!("distiller: module loaded, registered by skill requirement check");
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { .. } => {
                let now = now_secs();
                if now - self.last_distill_time < self.distill_interval {
                    return Ok(EventResponse::Pass);
                }

                // 通过 ctx.engine 定位 SkillModule 并蒸馏
                let skill_mod_id = ctx.engine.find_by_capability(&"skill.practice".into())
                    .unwrap_or_default();
                if skill_mod_id.is_empty() {
                    return Ok(EventResponse::Pass);
                }

                // 通过 raw data 接口获取 SkillModule
                let raw = ctx.engine.query_module_raw_data(&skill_mod_id);
                if raw.is_none() {
                    return Ok(EventResponse::Pass);
                }

                // 由于 query_module_raw_data 返回 raw pointer，无法安全调方法
                // 蒸馏逻辑由外部（main.rs/gateway）通过 with_module_mut 触发
                tracing::debug!("distiller: ready, skill module: {}", skill_mod_id);
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { None }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { None }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
