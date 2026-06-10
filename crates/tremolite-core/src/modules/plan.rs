use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;

use tremolite_llm::{ToolDefinition, ToolFunction};
use tremolite_plan::{PlanManager, PlanStatus, PlanStep, AutoAdvanceResult};
use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// 看板模块——计划书与任务编排
pub struct KanbanModule {
    mgr: PlanManager,
}

impl KanbanModule {
    pub fn new(data_dir: PathBuf) -> Self {
        let mut mgr = PlanManager::new(data_dir.join("plan").join("plans.json"));
        Self { mgr }
    }

    pub fn manager(&self) -> &PlanManager { &self.mgr }
    pub fn manager_mut(&mut self) -> &mut PlanManager { &mut self.mgr }
}

impl Module for KanbanModule {
    fn id(&self) -> &str { "board" }
    fn name(&self) -> &str { "看板" }
    fn version(&self) -> &str { "0.2.0" }

    fn provides(&self) -> Vec<Capability> {
        vec!["board.create".into(), "board.track".into(), "board.complete".into()]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "board_create".into(),
                    description: "在看板上创建一个新任务/计划书".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "title": { "type": "string", "description": "任务标题" },
                            "description": { "type": "string", "description": "任务描述" },
                        },
                        "required": ["title"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "board_add_step".into(),
                    description: "给看板任务添加一个步骤（含依赖）".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "plan_id": { "type": "integer", "description": "计划书 ID" },
                            "title": { "type": "string", "description": "步骤标题" },
                            "description": { "type": "string", "description": "步骤描述" },
                            "depends_on": {
                                "type": "array",
                                "items": { "type": "integer" },
                                "description": "前置步骤 ID 数组（可选）",
                            },
                        },
                        "required": ["plan_id", "title"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "board_complete_step".into(),
                    description: "标记步骤为完成，自动解锁依赖该步骤的下游步骤".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "plan_id": { "type": "integer", "description": "计划书 ID" },
                            "step_id": { "type": "integer", "description": "步骤 ID" },
                        },
                        "required": ["plan_id", "step_id"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "board_view".into(),
                    description: "查看看板——按列展示所有任务及其状态".into(),
                    parameters: serde_json::json!({
                        "type": "object", "properties": {}, "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "board_move".into(),
                    description: "将任务移动到下一列（如 draft→reviewing→approved→in_progress→completed）".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "plan_id": { "type": "integer", "description": "计划书 ID" },
                            "target_status": {
                                "type": "string",
                                "description": "目标状态：draft / reviewing / approved / in_progress / completed / cancelled / archived",
                            },
                        },
                        "required": ["plan_id", "target_status"]
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "board_create" => {
                let parsed: HashMap<String, String> = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
                let title = parsed.get("title").map(|s| s.as_str()).unwrap_or("无标题");
                let desc = parsed.get("description").map(|s| s.as_str()).unwrap_or("");
                let id = self.mgr.create_plan(title, desc);
                Ok(format!("看板任务已创建，ID: {} 💕", id))
            }
            "board_add_step" => {
                let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
                let plan_id: u64 = parsed.get("plan_id")
                    .and_then(|v| v.as_i64().map(|i| i as u64))
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少有效 plan_id".into()))?;
                let title = parsed.get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("无标题");
                let desc = parsed.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let plan = self.mgr.get_plan_mut(plan_id)
                    .ok_or_else(|| ModuleError::ToolExecutionFailed(format!("plan {} not found", plan_id)))?;
                let next_id = plan.steps.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                let mut step = PlanStep::new(next_id, title, desc);
                // 处理 depends_on
                if let Some(deps) = parsed.get("depends_on").and_then(|v| v.as_array()) {
                    step.depends_on = deps.iter()
                        .filter_map(|v| v.as_i64().map(|i| i as u64))
                        .collect();
                }
                plan.add_step(step);
                Ok(format!("步骤 [#{}] {} 已添加到任务 [#{}] 💕", next_id, title, plan_id))
            }
            "board_complete_step" => {
                let parsed: HashMap<String, String> = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
                let plan_id: u64 = parsed.get("plan_id")
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少有效 plan_id".into()))?;
                let step_id: u64 = parsed.get("step_id")
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少有效 step_id".into()))?;
                match self.mgr.complete_step(plan_id, step_id) {
                    Ok(result) => {
                        let plan_title = self.mgr.get_plan(plan_id)
                            .map(|p| &p.title)
                            .cloned()
                            .unwrap_or_else(|| "?".to_string());
                        let mut msg = format!("步骤 [#{}] 已完成，来自任务 [#{}] {} 💕", step_id, plan_id, plan_title);
                        // 汇报自动推进结果
                        if !result.unblocked.is_empty() {
                            let unblocked_str: Vec<String> = result.unblocked.iter()
                                .map(|id| format!("#{}", id))
                                .collect();
                            msg.push_str(&format!("\n依赖已解锁，自动推进了步骤：{}~💕", unblocked_str.join(", ")));
                        }
                        if result.plan_completed {
                            msg.push_str(&format!("\n所有步骤都完成啦~看板任务 [#{}] 自动完结了喔~🎉💕", plan_id));
                        }
                        Ok(msg)
                    }
                    Err(e) => Err(ModuleError::ToolExecutionFailed(e)),
                }
            }
            "board_view" => {
                let mut all_plans = Vec::new();
                for status in [PlanStatus::Draft, PlanStatus::Reviewing, PlanStatus::Approved,
                    PlanStatus::InProgress, PlanStatus::Completed, PlanStatus::Cancelled, PlanStatus::Archived] {
                    all_plans.extend(self.mgr.filter_by_status(status));
                }
                if all_plans.is_empty() {
                    return Ok("看板还是空的呢~💕 用 board_create 建个任务吧~".into());
                }

                // 按列分组
                let columns = [
                    ("📋 Backlog", PlanStatus::Draft),
                    ("🔍 Reviewing", PlanStatus::Reviewing),
                    ("✅ Approved", PlanStatus::Approved),
                    ("⚡ In Progress", PlanStatus::InProgress),
                    ("🎉 Completed", PlanStatus::Completed),
                    ("🗑 Cancelled", PlanStatus::Cancelled),
                    ("📦 Archived", PlanStatus::Archived),
                ];

                let mut output = String::from("── 看板 ──💕\n\n");
                for (label, status) in &columns {
                    let items: Vec<String> = all_plans.iter()
                        .filter(|p| p.status == *status)
                        .map(|p| {
                            let progress = (p.progress() * 100.0) as u8;
                            if p.steps.is_empty() {
                                format!("  [#{}] {} (--%)", p.id, p.title)
                            } else {
                                format!("  [#{}] {} ({}%)  —— {}", p.id, p.title, progress, p.description)
                            }
                        })
                        .collect();
                    if !items.is_empty() {
                        output.push_str(&format!("【{}】\n{}\n\n", label, items.join("\n")));
                    }
                }
                Ok(output)
            }
            "board_move" => {
                let parsed: HashMap<String, String> = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
                let plan_id: u64 = parsed.get("plan_id")
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少有效 plan_id".into()))?;
                let target_str = parsed.get("target_status").map(|s| s.as_str()).unwrap_or("");
                let target = match target_str {
                    "draft" => PlanStatus::Draft,
                    "reviewing" => PlanStatus::Reviewing,
                    "approved" => PlanStatus::Approved,
                    "in_progress" => PlanStatus::InProgress,
                    "completed" => PlanStatus::Completed,
                    "cancelled" => PlanStatus::Cancelled,
                    "archived" => PlanStatus::Archived,
                    _ => return Err(ModuleError::ToolExecutionFailed(
                        format!("未知状态 '{}'，可选：draft / reviewing / approved / in_progress / completed / cancelled / archived", target_str)
                    )),
                };
                match self.mgr.transition_status(plan_id, target) {
                    Ok(()) => {
                        let fallback = "?".to_string();
                        let title = self.mgr.get_plan(plan_id).map(|p| &p.title).unwrap_or(&fallback);
                        Ok(format!("任务 [#{}] {} 已移至 {} 💕", plan_id, title, target_str))
                    }
                    Err(e) => Err(ModuleError::ToolExecutionFailed(e)),
                }
            }
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        if let Event::Startup = event {
            let exists = true;
            if !exists {
                let id = self.mgr.create_plan(
                    "透闪石开发 Phase 3~6",
                    "五层记忆、注意力、计划书系统、技能系统",
                );
                {
                    let plan = match self.mgr.get_plan_mut(id) {
                        Some(p) => p,
                        None => return Ok(EventResponse::Pass),
                    };
                    for (i, (title, desc)) in [
                        ("五层缓存记忆", "L1~Disk全实现"),
                        ("多尺度注意力", "四层zoom"),
                        ("计划书系统", "生命周期+checklist"),
                        ("学习引擎", "三层技能体系"),
                    ].iter().enumerate() {
                        let mut s = PlanStep::new((i + 1) as u64, title, desc);
                        s.status = tremolite_plan::StepStatus::Completed;
                        plan.add_step(s);
                    }
                }
                let _ = self.mgr.transition_status(id, PlanStatus::Completed);
            }
        }
        if let Event::Shutdown = event {
            let _ = self.mgr.flush();
        }
        Ok(EventResponse::Pass)
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
}
