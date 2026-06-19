/// 技能蒸馏器——从 practice_log 中发现高频模式，生成体系化技能
///
/// 设计参考 Hermes 反思系统：
/// 1. 定时（每小时）读取实践日志
/// 2. 用 LLM 分析高频模式
/// 3. 生成新技能并注册到 skill 模块

use std::any::Any;
use std::sync::Arc;

use tremolite_core::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError, ToolDefinition,
};
use tremolite_core::SkillModule;

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
        Self { llm, last_distill_time: 0, distill_interval: 3600 }
    }
}

impl Module for DistillerModule {
    fn id(&self) -> &str { "distiller" }
    fn name(&self) -> &str { "技能蒸馏器" }
    fn version(&self) -> &str { "0.2.0" }

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
                tracing::info!("distiller: module loaded, will distill every {}s", self.distill_interval);
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { response: _ } => {
                let now = now_secs();
                if now - self.last_distill_time < self.distill_interval {
                    return Ok(EventResponse::Pass);
                }
                self.last_distill_time = now;

                // 1. 获取 practice_log
                let log_text = ctx.engine.with_module("skill", |m| {
                    if let Some(skill) = m.as_any().and_then(|a| a.downcast_ref::<SkillModule>()) {
                        let engine = skill.engine();
                        let log: Vec<_> = engine.get_practice_log()
                            .iter()
                            .rev()
                            .take(30)
                            .map(|r| format!("  - [{}] {}: {} (success={})",
                                r.timestamp, r.skill_id, r.context, r.success))
                            .collect();
                        log.join("\n")
                    } else {
                        String::new()
                    }
                }).unwrap_or_default();

                if log_text.is_empty() {
                    tracing::debug!("distiller: no practice log yet, skipping");
                    return Ok(EventResponse::Pass);
                }

                // 2. 构造 LLM prompt
                let prompt = format!(
                    r#"你是一个技能蒸馏器。分析以下实践日志，提取高频行为模式，生成新的原子技能定义。

最近实践记录：
{log_text}

请基于以上数据分析，提出最多3个新的原子技能建议。
每个技能必须包含以下字段，用 JSON 格式输出（不要用 markdown 代码块包装，只输出纯 JSON 数组）：

[
  {{
    "id": "英文标识符，如 pattern-analysis",
    "name": "中文技能名",
    "category": "技能类别（如 communication, analysis, coding）",
    "description": "一句话描述",
    "body": "详细说明这个技能应该怎么用，什么场景下触发"
  }}
]

如果没有需要新增的技能，输出空数组 []。"#
                );

                // 3. 调用 LLM
                let result = match (self.llm)(&prompt) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::warn!("distiller: LLM call failed: {}", e);
                        return Ok(EventResponse::Pass);
                    }
                };

                // 4. 解析输出，注册新技能
                let trimmed = result.trim()
                    .trim_start_matches("```json").trim_start_matches("```")
                    .trim_end_matches("```").trim();
                if let Ok(skills) = serde_json::from_str::<Vec<SkillProposal>>(trimmed) {
                    let count = skills.len();
                    for sk in &skills {
                        ctx.engine.with_module("skill", |m| {
                            if let Some(skill_mod) = m.as_any_mut()
                                .and_then(|a| a.downcast_mut::<SkillModule>())
                            {
                                let _ = skill_mod.create_skill_file(
                                    &sk.id, &sk.name, &sk.category,
                                    &sk.description, &sk.body,
                                );
                            }
                        });
                    }
                    if count > 0 {
                        tracing::info!("distiller: created {} new skill(s)", count);
                    } else {
                        tracing::debug!("distiller: no new skills proposed");
                    }
                } else {
                    tracing::warn!("distiller: failed to parse LLM response: {}",
                        trimmed.chars().take(200).collect::<String>());
                }

                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { Some(self) }
}

#[derive(serde::Deserialize)]
struct SkillProposal {
    id: String,
    name: String,
    category: String,
    description: String,
    body: String,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
