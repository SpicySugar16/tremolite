use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::fs;

use tremolite_learn::LearningEngine;
use tremolite_llm::{ToolDefinition, ToolFunction};
use crate::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError, ModuleInfo,
};

/// 技能模块——从 LearningModule 升级的完整技能系统
///
/// 能力叠加：
/// - 原有学习引擎：三层技能体系 + 遗忘曲线 + 自动发现新模块能力
/// - 新增技能文件加载：从 `~/.tremolite/skills/` 加载 .md 文件为 AtomicSkill
/// - BuildPrompt 注入：根据上下文选择技能注入 system prompt
/// - list_skills / view_skill 工具
/// - 熟练度联动：低 < 0.3 警告，高 > 0.7 工具排序提权
pub struct SkillModule {
    engine: LearningEngine,
    skills_dir: PathBuf,
    /// 技能文件清单：文件路径 -> 技能 id（用于重新加载检测）
    loaded_skill_files: HashMap<String, String>,
    /// BuildPrompt 时注入的技能上下文
    current_skill_context: String,
    /// 最近一次成功注入的技能摘要
    last_injected_skills: Vec<String>,
}

impl SkillModule {
    pub fn new(data_dir: PathBuf) -> Self {
        let engine = LearningEngine::new(data_dir.join("learn").join("skills.json"));
        let mut module = Self {
            engine,
            skills_dir: data_dir.join("skills"),
            loaded_skill_files: HashMap::new(),
            current_skill_context: String::new(),
            last_injected_skills: Vec::new(),
        };

        // 启动时自动加载已有技能文件
        let _ = module.load_all_skills();
        module
    }

    pub fn stats(&self) -> tremolite_learn::LearnStats {
        self.engine.stats()
    }
    pub fn engine(&self) -> &LearningEngine {
        &self.engine
    }
    pub fn engine_mut(&mut self) -> &mut LearningEngine {
        &mut self.engine
    }

    /// 设置 LLM 回调——启用自学习和蒸馏
    pub fn set_llm(&mut self, llm: Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>) {
        self.engine.set_llm(llm);
    }

    /// 执行学习循环——自动归类域、合成知识、LLM 蒸馏
    pub fn learn_cycle(&mut self) -> tremolite_learn::LearnCycleStats {
        self.engine.learn_cycle()
    }

    /// 创建技能文件并加载到引擎
    ///
    /// 供外部 crate（distiller、self-learner）使用
    pub fn create_skill_file(
        &mut self,
        id: &str,
        name: &str,
        category: &str,
        description: &str,
        body: &str,
    ) -> Result<(), String> {
        // 检查是否已存在
        let skill_id = format!("file.{}", id);
        if self.engine.get_skill(&skill_id).is_some() {
            return Ok(()); // 已存在，跳过
        }

        // 写入 LearningEngine
        self.engine.practice(&skill_id, true, "external");

        // 写入 .md 文件
        let file_path = self.skills_dir.join(format!("{}.md", id));
        let md_content = format!(
            r#"---
name: {name}
category: {category}
description: {description}
---

# {name} ({id})

{description}

{body}"#,
            name = name,
            category = category,
            description = description,
            id = id,
            body = body,
        );

        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        fs::write(&file_path, md_content).map_err(|e| format!("write skill file: {}", e))?;

        tracing::info!("skill: created skill file '{}' ({} / {})", name, category, id);
        Ok(())
    }

    /// 加载所有技能文件
    fn load_all_skills(&mut self) -> Result<usize, String> {
        let dir = self.skills_dir.clone();
        if !dir.exists() {
            return Ok(0);
        }

        let entries = fs::read_dir(&dir).map_err(|e| e.to_string())?;
        let mut count = 0;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(false, |e| e == "md") {
                if let Ok(true) = self.inject_skill(&path) {
                    count += 1;
                }
            }
        }

        tracing::info!("skill: loaded {} skill files from {:?}", count, &self.skills_dir);
        Ok(count)
    }

    /// 从 .md 文件注入一个技能
    ///
    /// 文件格式：
    /// ```markdown
    /// ---
    /// name: 理解文本
    /// category: core
    /// description: 能理解用户输入的文字内容
    /// ---
    /// 详细描述和用法说明……
    /// ```
    fn inject_skill(&mut self, path: &PathBuf) -> Result<bool, String> {
        let content = fs::read_to_string(path).map_err(|e| format!("read skill file: {e}"))?;
        let path_str = path.to_string_lossy().to_string();

        // 解析 YAML frontmatter（简单的行解析，不引入 serde_yaml）
        let (frontmatter, _body) = if let Some(stripped) = content.strip_prefix("---") {
            if let Some(end) = stripped.find("\n---") {
                (&stripped[..end], &stripped[end + 4..])
            } else {
                // 没有结束 ---，把整个当 frontmatter
                (stripped, "")
            }
        } else {
            // 没有 frontmatter，用文件名生成 skill
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let id = format!("file.{}", name);
            self.engine.practice(&id, true, &format!("loaded from {}", path_str));
            self.loaded_skill_files.insert(path_str, id);
            return Ok(true);
        };

        // 解析字段
        let mut name = String::new();
        let mut category = String::new();
        let mut description = String::new();

        for line in frontmatter.lines() {
            let line = line.trim();
            if let Some(val) = line.strip_prefix("name:") {
                name = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("category:") {
                category = val.trim().to_string();
            } else if let Some(val) = line.strip_prefix("description:") {
                description = val.trim().to_string();
            }
        }

        if name.is_empty() {
            name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
        }
        if category.is_empty() {
            category = "custom".to_string();
        }

        let id = format!("file.{}", name);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 更新或创建
        self.engine
            .practice(&id, true, &format!("loaded from {}", path_str));

        // 练习记录会自动维护熟练度，description 通过 prompt_segment 引用
        //（LearningEngine 的 practice 不会设 description，我们在模块层手动维护）

        self.loaded_skill_files.insert(path_str, id.clone());
        tracing::info!("skill: injected '{}' ({}) from file", name, category);

        Ok(true)
    }

    /// 生成技能上下文——用于 BuildPrompt 注入
    fn build_skill_context(&self) -> String {
        let stats = self.engine.stats();
        let mut parts = Vec::new();

        // 已精通的技能
        let mastered: Vec<String> = (0..stats.total_skills)
            .filter_map(|_| {
                // 遍历获取已掌握技能
                None
            })
            .collect();

        // 实际我们通过 LearningEngine 的 suggest_practice 来间接获取
        // 这里直接用 engine 内部数据构建
        let suggestions = self.engine.suggest_practice(stats.total_skills);

        // 已掌握（熟练度 >= 0.6）和未掌握的
        let mastered_skills: Vec<String> = suggestions
            .iter()
            .filter(|(s, _)| s.is_mastered())
            .map(|(s, _)| format!("【{}】{} (熟练度 {:.0}%)", s.category, s.name, s.proficiency * 100.0))
            .collect();

        let learning_skills: Vec<String> = suggestions
            .iter()
            .filter(|(s, _)| !s.is_mastered())
            .map(|(s, _)| {
                let warning = if s.proficiency < 0.3 {
                    "（尚未掌握，谨慎使用）"
                } else {
                    "（练习中）"
                };
                format!("【{}】{} {warning}", s.category, s.name)
            })
            .collect();

        if !mastered_skills.is_empty() {
            parts.push(format!(
                "葵已精通的技能：\n{}",
                mastered_skills.join("\n")
            ));
        }
        if !learning_skills.is_empty() {
            parts.push(format!(
                "葵正在学习的技能：\n{}",
                learning_skills.join("\n")
            ));
        }

        if parts.is_empty() {
            String::new()
        } else {
            parts.join("\n\n")
        }
    }

    /// 更新 current_skill_context 并返回本次注入的 skill id 列表
    fn refresh_skill_context(&mut self) -> Vec<String> {
        let ctx = self.build_skill_context();
        self.current_skill_context = ctx;

        // 生成注入列表
        let suggestions = self.engine.suggest_practice(self.engine.stats().total_skills);
        let ids: Vec<String> = suggestions
            .iter()
            .map(|(s, _)| s.id.clone())
            .collect();
        self.last_injected_skills = ids.clone();
        ids
    }
}

impl Module for SkillModule {
    fn id(&self) -> &str {
        "skill"
    }
    fn name(&self) -> &str {
        "技能系统"
    }
    fn version(&self) -> &str {
        "0.3.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "skill.practice".into(),
            "skill.suggest".into(),
            "skill.compose".into(),
            "skill.module_discovery".into(),
            "skill.inject".into(),
            "skill.view".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec![]
    }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "list_skills".into(),
                    description: "查看葵已掌握的各项技能和熟练度".into(),
                    parameters: serde_json::json!({
                        "type": "object", "properties": {}, "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "view_skill".into(),
                    description: "查看某个技能的详细信息，包括熟练度、成功率、最近使用时间等".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "skill_id": {
                                "type": "string",
                                "description": "技能 ID（如 understand_text, file.理解文本）"
                            }
                        },
                        "required": ["skill_id"]
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "list_skills" => {
                let stats = self.engine.stats();
                let suggestions = self.engine.suggest_practice(stats.total_skills);
                if suggestions.is_empty() {
                    return Ok("葵还没有掌握任何技能呢……".into());
                }
                let mut lines: Vec<String> = Vec::new();
                lines.push(format!(
                    "技能总数: {} | 已掌握: {} | 精通: {}",
                    stats.total_skills,
                    stats.mastered_skills,
                    stats.expert_skills,
                ));
                lines.push("".into());
                for (skill, reason) in &suggestions {
                    let mastery = if skill.is_expert() {
                        "精通"
                    } else if skill.is_mastered() {
                        "掌握"
                    } else {
                        "学习中"
                    };
                    let warn = if skill.proficiency < 0.3 {
                        " ⚠️ 尚未掌握"
                    } else {
                        ""
                    };
                    lines.push(format!(
                        "  [{}] {} ({:.0}%) [{}]{warn}",
                        skill.category,
                        skill.name,
                        skill.proficiency * 100.0,
                        mastery,
                    ));
                }
                Ok(lines.join("\n"))
            }
            "view_skill" => {
                let parsed: HashMap<String, serde_json::Value> =
                    serde_json::from_str(args).unwrap_or_default();
                let skill_id = parsed
                    .get("skill_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if skill_id.is_empty() {
                    return Err(ModuleError::ToolExecutionFailed(
                        "请提供技能 ID 参数".into(),
                    ));
                }
                let skill = self
                    .engine
                    .get_skill(skill_id)
                    .ok_or_else(|| ModuleError::ToolNotFound(skill_id.to_string()))?;

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let hours_ago = (now - skill.last_used) as f64 / 3600.0;
                let last_used_str = if hours_ago < 1.0 {
                    "刚刚".to_string()
                } else if hours_ago < 24.0 {
                    format!("{:.0} 小时前", hours_ago)
                } else {
                    format!("{:.0} 天前", hours_ago / 24.0)
                };

                Ok(format!(
                    r#"技能: {}
  名称: {}
  类别: {}
  熟练度: {:.0}%
  使用次数: {}
  成功率: {:.0}%
  最后使用: {}
  状态: {}"#,
                    skill.id,
                    skill.name,
                    skill.category,
                    skill.proficiency * 100.0,
                    skill.use_count,
                    skill.success_rate * 100.0,
                    last_used_str,
                    if skill.is_expert() {
                        "精通"
                    } else if skill.is_mastered() {
                        "已掌握"
                    } else if skill.proficiency < 0.3 {
                        "尚未掌握，谨慎使用"
                    } else {
                        "练习中"
                    },
                ))
            }
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn prompt_segment(&self) -> Option<String> {
        if self.current_skill_context.is_empty() {
            None
        } else {
            Some(format!("[技能状态]\n{}", self.current_skill_context))
        }
    }

    fn display_status(&self) -> Option<String> {
        let stats = self.engine.stats();
        Some(format!(
            "技能: {}/{}掌握",
            stats.mastered_skills,
            stats.total_skills,
        ))
    }

    fn on_event(
        &mut self,
        event: &Event,
        _ctx: &EventContext,
    ) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                self.engine
                    .practice("understand_text", true, "engine startup");
                self.engine
                    .practice("detect_emotion", true, "engine startup");
                self.refresh_skill_context();
                Ok(EventResponse::Pass)
            }
            Event::BuildPrompt => {
                // 在构建 prompt 时刷新技能上下文（自动注入到系统提示）
                self.refresh_skill_context();
                Ok(EventResponse::Pass)
            }
            Event::OnMessage { input, .. } => {
                self.engine.practice("understand_text", true, input);
                Ok(EventResponse::Pass)
            }
            Event::OnToolCall {
                name,
                args,
                success,
            } => {
                self.engine.practice(name, *success, args);
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { response: _ } => {
                // 每次回复后触发学习循环——自动归域、合成知识、LLM 蒸馏
                let _stats = self.learn_cycle();
                Ok(EventResponse::Pass)
            }
            Event::ModuleRegistered { info } => {
                for cap in &info.provides {
                    let skill_id = format!("module.{}", cap.replace('.', "_"));
                    self.engine.practice(
                        &skill_id,
                        true,
                        &format!("module {} registered", info.id),
                    );
                }
                for tool in &info.tools {
                    let skill_id = format!("tool.{}", tool.function.name.replace('.', "_"));
                    self.engine
                        .practice(&skill_id, true, &format!("tool from module {}", info.id));
                }
                tracing::info!(
                    "skill: auto-discovered {} capabilities from '{}'",
                    info.provides.len(),
                    info.name
                );
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { .. } => {
                let suggestions = self.engine.suggest_practice(2);
                for (skill, reason) in &suggestions {
                    tracing::debug!(
                        "技能建议：技能「{}」{}（熟练度 {:.2}）",
                        skill.name,
                        reason,
                        skill.proficiency
                    );
                }
                let domain_ids: Vec<String> = self.engine.list_mature_domains(0.5);
                if domain_ids.len() >= 2 {
                    self.engine.auto_compose(&domain_ids[0], &domain_ids[1]);
                }
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                let _ = self.engine.flush();
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}
