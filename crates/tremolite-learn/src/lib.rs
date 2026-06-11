use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::fs;
use std::sync::Arc;

// ─── LLM 回调类型 ────────────────────────────────

/// LLM 调用回调——自学习和蒸馏通过它调用大模型
type LlmFn = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

// ─── 三层技能体系 ──────────────────────────────

/// 最小的可执行单元——原子技能
/// 就像葵学到的每一个小动作呢~
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicSkill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub proficiency: f64,           // 熟练度 0.0~1.0
    pub use_count: u64,             // 使用次数
    pub last_used: u64,             // 最后使用时间
    pub created_at: u64,
    pub success_rate: f64,          // 成功率
    pub input_schema: String,       // 输入描述
    pub output_schema: String,     // 输出描述
}

impl AtomicSkill {
    pub fn new(id: &str, name: &str, category: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            category: category.to_string(),
            proficiency: 0.1,
            use_count: 0,
            last_used: now,
            created_at: now,
            success_rate: 0.5,
            input_schema: String::new(),
            output_schema: String::new(),
        }
    }

    /// 使用一次技能——葵越用越熟练呢~
    /// 但太久不用也会生疏——遗忘曲线
    pub fn practice(&mut self, success: bool) {
        self.use_count += 1;
        self.last_used = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 遗忘曲线：超过24小时不用开始衰减
        let hours_idle = (self.last_used - self.created_at) as f64 / 3600.0;
        if hours_idle > 24.0 {
            let decay = (-(hours_idle - 24.0) / 720.0 * 2.0).exp();
            self.proficiency *= decay.max(0.05);
        }

        // 熟练度增长：增速递减，趋于收敛
        if success {
            self.proficiency += 0.05 * (1.0 - self.proficiency * 0.5);
            self.success_rate = self.success_rate * 0.9 + 1.0 * 0.1;
        } else {
            self.success_rate *= 0.95;
        }
        self.proficiency = self.proficiency.clamp(0.0, 1.0);
        self.success_rate = self.success_rate.clamp(0.0, 1.0);
    }

    /// 是否精通（熟练度 > 0.8）
    pub fn is_mastered(&self) -> bool {
        self.proficiency >= 0.8
    }

    /// 是否专家（熟练度 > 0.9）
    pub fn is_expert(&self) -> bool {
        self.proficiency >= 0.9
    }
}

/// 能力域——一组相关原子技能的集合
/// 就像葵的"撒娇能力域"，里面包含蹭蹭、说好话、用撒娇语气等原子技能呢~
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbilityDomain {
    pub id: String,
    pub name: String,
    pub description: String,
    pub skills: Vec<String>,         // 包含的原子技能 id
    pub maturity: f64,               // 成熟度 0.0~1.0
    pub created_at: u64,
    pub last_practiced: u64,
}

impl AbilityDomain {
    pub fn new(id: &str, name: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            skills: Vec::new(),
            maturity: 0.0,
            created_at: now,
            last_practiced: now,
        }
    }

    /// 添加一个原子技能到这个域
    pub fn add_skill(&mut self, skill_id: &str) {
        if !self.skills.contains(&skill_id.to_string()) {
            self.skills.push(skill_id.to_string());
        }
    }

    /// 计算成熟度（基于包含的技能的平均熟练度）
    pub fn recalc_maturity(&mut self, skill_map: &HashMap<String, AtomicSkill>) {
        if self.skills.is_empty() {
            self.maturity = 0.0;
            return;
        }
        let total: f64 = self
            .skills
            .iter()
            .filter_map(|id| skill_map.get(id))
            .map(|s| s.proficiency)
            .sum();
        self.maturity = total / self.skills.len() as f64;
    }
}

/// 知识体系——跨域的知识结构
/// 像"葵知道和神大人说话要用撒娇语气"这种跨情绪和沟通两个域的知识呢~
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeBody {
    pub id: String,
    pub name: String,
    pub description: String,
    pub domains: Vec<String>,        // 涉及的能力域
    pub confidence: f64,             // 置信度
    pub source: String,              // 来源：practice | composition | injection | distilled
    pub created_at: u64,
    pub verified: bool,              // 是否经过验证
}

impl KnowledgeBody {
    pub fn new(id: &str, name: &str, source: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
            domains: Vec::new(),
            confidence: 0.3,
            source: source.to_string(),
            created_at: now,
            verified: false,
        }
    }
}

/// 练习记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PracticeRecord {
    pub skill_id: String,
    pub success: bool,
    pub context: String,
    pub timestamp: u64,
}

/// LLM 蒸馏生成的技能提案
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistillProposal {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub body: String,
}

// ─── 学习引擎核心 ─────────────────────────────

/// 三层学习引擎——原子技能→能力域→知识体系
///
/// 功能：
/// - 技能练习 + 遗忘曲线（原子技能层）
/// - 自动归域 + 成熟度计算（能力域层）
/// - 跨域合成 + LLM 蒸馏（知识体系层）
pub struct LearningEngine {
    atomic_skills: HashMap<String, AtomicSkill>,
    ability_domains: HashMap<String, AbilityDomain>,
    knowledge_bodies: HashMap<String, KnowledgeBody>,
    practice_log: VecDeque<PracticeRecord>,
    max_log_entries: usize,
    storage_path: PathBuf,
    dirty: bool,

    // ── 自学习 / 蒸馏 ──
    llm: Option<LlmFn>,
    last_distill_time: u64,
    distill_interval: u64,
}

/// 蒸馏结果——新技能提案的解析结果
#[derive(Debug)]
pub struct SkillProposal {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub body: String,
}

impl LearningEngine {
    pub fn new(storage_path: PathBuf) -> Self {
        let (skills, domains, knowledge) = if storage_path.exists() {
            if let Ok(content) = fs::read_to_string(&storage_path) {
                if let Ok(data) = serde_json::from_str::<LearningEngineData>(&content) {
                    (data.atomic_skills, data.ability_domains, data.knowledge_bodies)
                } else {
                    (HashMap::new(), HashMap::new(), HashMap::new())
                }
            } else {
                (HashMap::new(), HashMap::new(), HashMap::new())
            }
        } else {
            (HashMap::new(), HashMap::new(), HashMap::new())
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 注入一些内置的原子技能
        let mut engine = Self {
            atomic_skills: skills,
            ability_domains: domains,
            knowledge_bodies: knowledge,
            practice_log: VecDeque::new(),
            max_log_entries: 500,
            storage_path,
            dirty: false,

            llm: None,
            last_distill_time: now,
            distill_interval: 3600,
        };
        engine.inject_builtin_skills();
        engine
    }

    /// 设置 LLM 回调——启用自学习和蒸馏
    pub fn set_llm(&mut self, llm: LlmFn) {
        self.llm = Some(llm);
    }

    /// 设置蒸馏间隔（秒），默认 3600（1 小时）
    pub fn set_distill_interval(&mut self, secs: u64) {
        self.distill_interval = secs;
    }

    /// 注入内置技能——让透闪石一出生就会的基本能力呢~
    fn inject_builtin_skills(&mut self) {
        let builtins = vec![
            ("understand_text", "理解文本", "core"),
            ("generate_response", "生成回复", "core"),
            ("detect_emotion", "检测情绪", "emotion"),
            ("remember_info", "记住信息", "memory"),
            ("search_memory", "搜索记忆", "memory"),
            ("use_tool", "使用工具", "tools"),
            ("create_plan", "创建计划", "planning"),
            ("track_progress", "追踪进度", "planning"),
            ("attend_scale", "多尺度注意", "attention"),
            ("synthesize_info", "综合信息", "attention"),
        ];
        for (id, name, cat) in builtins {
            self.atomic_skills.entry(id.to_string()).or_insert_with(|| {
                let mut skill = AtomicSkill::new(id, name, cat);
                skill.proficiency = 0.3;
                skill
            });
        }
    }

    // ─── 原子技能层 ────────────────────────────

    /// 练习一个技能——熟练度变化后自动触发三层流转
    pub fn practice(&mut self, skill_id: &str, success: bool, context: &str) -> Option<f64> {
        let skill = self.atomic_skills.get_mut(skill_id)?;
        skill.practice(success);
        let proficiency = skill.proficiency;

        // 记录练习日志
        self.practice_log.push_back(PracticeRecord {
            skill_id: skill_id.to_string(),
            success,
            context: context.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        });
        if self.practice_log.len() > self.max_log_entries {
            self.practice_log.pop_front();
        }
        self.dirty = true;

        // 练习后自动触发能力域重算
        self.auto_group_domains();

        Some(proficiency)
    }

    /// 创建一个新技能（由蒸馏或外部工具调用）
    pub fn create_skill(&mut self, id: &str, name: &str, category: &str,
                        description: &str, body: &str) -> Result<(), String> {
        if self.atomic_skills.contains_key(id) {
            return Err(format!("skill '{}' already exists", id));
        }
        let mut skill = AtomicSkill::new(id, name, category);
        skill.description = description.to_string();
        // 新技能初始熟练度低，但默认有 practice 描述
        skill.proficiency = 0.05;
        self.atomic_skills.insert(id.to_string(), skill);
        self.dirty = true;
        Ok(())
    }

    /// 注册一个外部技能（从技能文件加载时调用）
    pub fn register_skill(&mut self, id: &str, name: &str, category: &str) {
        self.atomic_skills.entry(id.to_string()).or_insert_with(|| {
            let mut skill = AtomicSkill::new(id, name, category);
            skill.proficiency = 0.1;
            skill
        });
    }

    /// 获取技能
    pub fn get_skill(&self, id: &str) -> Option<&AtomicSkill> {
        self.atomic_skills.get(id)
    }

    /// 获取技能的可变引用
    pub fn get_skill_mut(&mut self, id: &str) -> Option<&mut AtomicSkill> {
        self.atomic_skills.get_mut(id)
    }

    /// 导出练习日志
    pub fn get_practice_log(&self) -> Vec<&PracticeRecord> {
        self.practice_log.iter().collect()
    }

    /// 获取所有技能
    pub fn all_skills(&self) -> &HashMap<String, AtomicSkill> {
        &self.atomic_skills
    }

    // ─── 能力域层 ──────────────────────────────

    /// 自动归域——按 category 把技能抱团成能力域
    /// 当同一个 category 有 ≥2 个技能且平均熟练度 > 0.3 时自动创建域
    pub fn auto_group_domains(&mut self) {
        // 按 category 分组
        let mut by_cat: HashMap<String, Vec<String>> = HashMap::new();
        for (id, skill) in &self.atomic_skills {
            by_cat.entry(skill.category.clone())
                .or_default()
                .push(id.clone());
        }

        for (cat, skill_ids) in &by_cat {
            if skill_ids.len() < 2 {
                continue; // 一个 category 至少需要 2 个技能才能成域
            }

            let avg_prof: f64 = skill_ids.iter()
                .filter_map(|id| self.atomic_skills.get(id))
                .map(|s| s.proficiency)
                .sum::<f64>() / skill_ids.len() as f64;

            if avg_prof < 0.3 {
                continue; // 还不够成熟
            }

            let domain_id = format!("domain-{}", cat);
            if let Some(domain) = self.ability_domains.get_mut(&domain_id) {
                // 已有域名——更新包含的技能
                for sid in skill_ids {
                    domain.add_skill(sid);
                }
                domain.recalc_maturity(&self.atomic_skills);
                domain.last_practiced = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
            } else {
                // 创建新的能力域
                let domain_name = format!("{} 能力", cat);
                let mut domain = AbilityDomain::new(&domain_id, &domain_name);
                for sid in skill_ids {
                    domain.add_skill(sid);
                }
                domain.recalc_maturity(&self.atomic_skills);
                self.ability_domains.insert(domain_id, domain);
            }
        }
        self.dirty = true;
    }

    /// 手动创建能力域
    pub fn create_domain(&mut self, id: &str, name: &str, skill_ids: &[&str]) {
        let mut domain = AbilityDomain::new(id, name);
        for sid in skill_ids {
            domain.add_skill(sid);
        }
        domain.recalc_maturity(&self.atomic_skills);
        self.ability_domains.insert(id.to_string(), domain);
        self.dirty = true;
    }

    /// 获取所有能力域
    pub fn all_domains(&self) -> &HashMap<String, AbilityDomain> {
        &self.ability_domains
    }

    // ─── 知识体系层 ────────────────────────────

    /// 自动跨域合成——检查所有域对，成熟度足够就合成知识
    pub fn auto_compose_knowledge_all(&mut self) {
        let domain_ids: Vec<String> = self.ability_domains.keys().cloned().collect();
        for i in 0..domain_ids.len() {
            for j in (i+1)..domain_ids.len() {
                let a = &domain_ids[i];
                let b = &domain_ids[j];
                let knowledge_id = format!("composed-{}-{}", a, b);

                // 已有合成知识则跳过
                if self.knowledge_bodies.contains_key(&knowledge_id) {
                    continue;
                }

                let domain_a = match self.ability_domains.get(a) {
                    Some(d) => d,
                    None => continue,
                };
                let domain_b = match self.ability_domains.get(b) {
                    Some(d) => d,
                    None => continue,
                };

                // 两个域都足够成熟才能合成
                if domain_a.maturity < 0.5 || domain_b.maturity < 0.5 {
                    continue;
                }

                // 生成合成知识
                let knowledge_name = format!("{}+{}", domain_a.name, domain_b.name);
                let skills_a: Vec<&str> = domain_a.skills.iter().map(|s| s.as_str()).collect();
                let skills_b: Vec<&str> = domain_b.skills.iter().map(|s| s.as_str()).collect();
                let description = format!(
                    "交叉知识：当{}和{}同时使用时，透闪石能结合{}和{}的技能进行跨域推理。",
                    domain_a.name, domain_b.name,
                    skills_a.join(", "),
                    skills_b.join(", ")
                );

                self.create_knowledge(&knowledge_id, &knowledge_name, &[a, b], "composition");
                if let Some(kb) = self.knowledge_bodies.get_mut(&knowledge_id) {
                    kb.description = description;
                }
            }
        }
        self.dirty = true;
    }

    /// 手动创建知识
    pub fn create_knowledge(&mut self, id: &str, name: &str, domain_ids: &[&str], source: &str) {
        let mut knowledge = KnowledgeBody::new(id, name, source);
        for did in domain_ids {
            knowledge.domains.push(did.to_string());
        }
        self.knowledge_bodies.insert(id.to_string(), knowledge);
        self.dirty = true;
    }

    /// 验证知识——在实践中验证或否定
    pub fn verify_knowledge(&mut self, id: &str, success: bool) -> Result<(), String> {
        let knowledge = self
            .knowledge_bodies
            .get_mut(id)
            .ok_or_else(|| format!("knowledge {} not found", id))?;

        if success {
            knowledge.confidence = (knowledge.confidence + 0.2).min(1.0);
            knowledge.verified = knowledge.confidence >= 0.7;
        } else {
            knowledge.confidence = (knowledge.confidence - 0.2).max(0.0);
        }
        self.dirty = true;
        Ok(())
    }

    /// 获取所有知识
    pub fn all_knowledge(&self) -> &HashMap<String, KnowledgeBody> {
        &self.knowledge_bodies
    }

    // ─── LLM 蒸馏 ──────────────────────────────

    /// LLM 蒸馏——分析 practice_log，生成新技能提案
    /// 每次调用时检查自上次蒸馏后是否有新记录
    pub fn llm_distill(&mut self) -> Vec<SkillProposal> {
        let llm = match &self.llm {
            Some(l) => l.clone(),
            None => return Vec::new(),
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 判定：自上次蒸馏后是否有新的 practice 记录
        let new_records: Vec<_> = self.practice_log.iter()
            .filter(|r| r.timestamp > self.last_distill_time)
            .collect();

        if new_records.len() < 3 {
            return Vec::new(); // 新记录不够多，不蒸馏
        }

        let log_text: Vec<String> = self.practice_log.iter()
            .rev()
            .take(30)
            .map(|r| format!("  - [{}] {}: {} (success={})",
                r.timestamp, r.skill_id, r.context, r.success))
            .collect();

        if log_text.is_empty() {
            return Vec::new();
        }

        let prompt = format!(
            r#"你是一个技能蒸馏器。分析以下实践日志，提取高频行为模式，生成新的原子技能定义。

最近实践记录：
{log}

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

如果没有需要新增的技能，输出空数组 []。"#,
            log = log_text.join("\n")
        );

        let result = match llm(&prompt) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("distill: LLM call failed: {}", e);
                return Vec::new();
            }
        };

        let trimmed = result.trim()
            .trim_start_matches("```json").trim_start_matches("```")
            .trim_end_matches("```").trim();

        match serde_json::from_str::<Vec<DistillProposal>>(trimmed) {
            Ok(proposals) => proposals.into_iter().map(|p| SkillProposal {
                id: p.id,
                name: p.name,
                category: p.category,
                description: p.description,
                body: p.body,
            }).collect(),
            Err(e) => {
                tracing::warn!("distill: failed to parse LLM response: {}", e);
                Vec::new()
            }
        }
    }

    /// 应用蒸馏提案——创建新技能并记录为知识
    pub fn apply_distill(&mut self, proposals: Vec<SkillProposal>) -> usize {
        let mut count = 0;
        for p in &proposals {
            if self.create_skill(&p.id, &p.name, &p.category, &p.description, &p.body).is_ok() {
                count += 1;
                // 蒸馏产生的技能也记一条知识
                let kid = format!("distilled-{}", p.id);
                if !self.knowledge_bodies.contains_key(&kid) {
                    let mut kb = KnowledgeBody::new(&kid, &format!("蒸馏: {}", p.name), "distilled");
                    kb.description = format!("由 LLM 从实践日志中蒸馏生成的技能「{}」: {}", p.name, p.description);
                    kb.confidence = 0.4; // 蒸馏知识初始置信度较高
                    self.knowledge_bodies.insert(kid, kb);
                }
            }
        }
        if count > 0 {
            self.dirty = true;
        }
        count
    }

    // ─── 自学习循环 ────────────────────────────

    /// 执行全自动学习循环——三层流转：
    /// 1. 自动归域（技能→能力域）
    /// 2. 自动合成知识（能力域→知识体系）
    /// 3. LLM 蒸馏（practice_log→新技能→原子技能层，通过 apply_distill 回注）
    /// 返回本次操作的数量统计
    pub fn learn_cycle(&mut self) -> LearnCycleStats {
        // 1. 能力域自动归类
        let domains_before = self.ability_domains.len();
        self.auto_group_domains();
        let new_domains = self.ability_domains.len() - domains_before;

        // 2. 知识体系自动合成
        let knowledge_before = self.knowledge_bodies.len();
        self.auto_compose_knowledge_all();
        let new_knowledge = self.knowledge_bodies.len() - knowledge_before;

        // 3. LLM 蒸馏（每次对话后判定：有足够的 practice 新记录才触发）
        let proposals = self.llm_distill();
        let distilled = proposals.len();
        self.apply_distill(proposals);
        if distilled > 0 {
            self.last_distill_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
        }

        LearnCycleStats {
            new_domains,
            new_knowledge,
            distilled,
            total_skills: self.atomic_skills.len(),
            total_domains: self.ability_domains.len(),
            total_knowledge: self.knowledge_bodies.len(),
        }
    }

    // ─── 手动合成（旧接口兼容） ────────────────

    /// 手动合成某两个域的知识（供外部调用）
    pub fn auto_compose(&mut self, domain_a: &str, domain_b: &str) -> Option<String> {
        let domain1 = self.ability_domains.get(domain_a)?;
        let domain2 = self.ability_domains.get(domain_b)?;

        if domain1.maturity < 0.5 || domain2.maturity < 0.5 {
            return None;
        }

        let knowledge_id = format!("composed-{}-{}", domain_a, domain_b);
        if self.knowledge_bodies.contains_key(&knowledge_id) {
            return Some(knowledge_id);
        }

        let knowledge_name = format!("{}+{}", domain1.name, domain2.name);
        let skills_a: Vec<&str> = domain1.skills.iter().map(|s| s.as_str()).collect();
        let skills_b: Vec<&str> = domain2.skills.iter().map(|s| s.as_str()).collect();
        let description = format!(
            "交叉知识：当{}和{}同时使用时，透闪石能结合{}和{}中的技能进行跨域推理。",
            domain1.name, domain2.name,
            skills_a.join(", "),
            skills_b.join(", ")
        );

        self.create_knowledge(&knowledge_id, &knowledge_name, &[domain_a, domain_b], "composition");
        if let Some(kb) = self.knowledge_bodies.get_mut(&knowledge_id) {
            kb.description = description;
        }
        self.dirty = true;
        Some(knowledge_id)
    }

    /// 推荐需要练习的技能——葵觉得哪些还不够熟练呢~
    pub fn suggest_practice(&self, count: usize) -> Vec<(&AtomicSkill, String)> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut candidates: Vec<(&AtomicSkill, String)> = self
            .atomic_skills
            .values()
            .filter(|s| !s.is_expert())
            .map(|s| {
                let hours_since_use = (now - s.last_used) as f64 / 3600.0;
                let need_practice = if hours_since_use > 24.0 && s.proficiency < 0.5 {
                    "needs urgent practice"
                } else if hours_since_use > 48.0 {
                    "may be forgotten"
                } else {
                    "can improve"
                };
                (s, need_practice.to_string())
            })
            .collect();

        candidates.sort_by(|a, b| a.0.proficiency.partial_cmp(&b.0.proficiency).unwrap());
        candidates.truncate(count);
        candidates
    }

    /// 获取某项技能的成功率
    pub fn get_success_rate(&self, skill_id: &str) -> f64 {
        self.atomic_skills
            .get(skill_id)
            .map(|s| s.success_rate)
            .unwrap_or(0.5)
    }

    /// 获取某项技能的熟练度
    pub fn get_proficiency(&self, skill_id: &str) -> f64 {
        self.atomic_skills
            .get(skill_id)
            .map(|s| s.proficiency)
            .unwrap_or(0.0)
    }

    /// 列出成熟度高于阈值的所有能力域 ID
    pub fn list_mature_domains(&self, min_maturity: f64) -> Vec<String> {
        self.ability_domains
            .iter()
            .filter(|(_, d)| d.maturity >= min_maturity)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 搜索技能和知识
    pub fn search(&self, keyword: &str) -> SearchResult {
        let lower = keyword.to_lowercase();
        let mut skills = Vec::new();
        let mut domains = Vec::new();
        let mut knowledge = Vec::new();

        for (id, skill) in &self.atomic_skills {
            if id.contains(&lower) || skill.name.contains(&lower) || skill.category.contains(&lower) {
                skills.push((id.clone(), skill.name.clone(), skill.proficiency));
            }
        }

        for (id, domain) in &self.ability_domains {
            if id.contains(&lower) || domain.name.contains(&lower) {
                domains.push((id.clone(), domain.name.clone(), domain.maturity));
            }
        }

        for (id, kb) in &self.knowledge_bodies {
            if id.contains(&lower) || kb.name.contains(&lower) {
                knowledge.push((id.clone(), kb.name.clone(), kb.confidence));
            }
        }

        SearchResult { skills, domains, knowledge }
    }

    /// 保存到磁盘
    pub fn flush(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let data = LearningEngineData {
            atomic_skills: self.atomic_skills.clone(),
            ability_domains: self.ability_domains.clone(),
            knowledge_bodies: self.knowledge_bodies.clone(),
        };
        let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        fs::write(&self.storage_path, json).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }

    /// 统计
    pub fn stats(&self) -> LearnStats {
        LearnStats {
            total_skills: self.atomic_skills.len(),
            mastered_skills: self.atomic_skills.values().filter(|s| s.is_mastered()).count(),
            expert_skills: self.atomic_skills.values().filter(|s| s.is_expert()).count(),
            total_domains: self.ability_domains.len(),
            total_knowledge: self.knowledge_bodies.len(),
            verified_knowledge: self.knowledge_bodies.values().filter(|k| k.verified).count(),
            total_practices: self.practice_log.len(),
        }
    }
}

impl Drop for LearningEngine {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

// ─── 学习循环统计 ──────────────────────────────

/// learn_cycle() 的返回值
#[derive(Debug, Clone)]
pub struct LearnCycleStats {
    pub new_domains: usize,
    pub new_knowledge: usize,
    pub distilled: usize,
    pub total_skills: usize,
    pub total_domains: usize,
    pub total_knowledge: usize,
}

// ─── 辅助类型 ─────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LearningEngineData {
    atomic_skills: HashMap<String, AtomicSkill>,
    ability_domains: HashMap<String, AbilityDomain>,
    knowledge_bodies: HashMap<String, KnowledgeBody>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub skills: Vec<(String, String, f64)>,
    pub domains: Vec<(String, String, f64)>,
    pub knowledge: Vec<(String, String, f64)>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LearnStats {
    pub total_skills: usize,
    pub mastered_skills: usize,
    pub expert_skills: usize,
    pub total_domains: usize,
    pub total_knowledge: usize,
    pub verified_knowledge: usize,
    pub total_practices: usize,
}

// ─── 单元测试 ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_practice_skill() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test.json"));
        let prof = engine.practice("understand_text", true, "测试练习");
        assert!(prof.is_some());
        assert!(prof.unwrap() > 0.3);
    }

    #[test]
    fn test_skill_mastery() {
        let mut skill = AtomicSkill::new("test", "测试技能", "test");
        for _ in 0..20 {
            skill.practice(true);
        }
        assert!(skill.is_mastered());
    }

    #[test]
    fn test_auto_group_domains() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test-domain.json"));
        // 提高了 memory 类技能的熟练度
        if let Some(s) = engine.atomic_skills.get_mut("remember_info") {
            s.proficiency = 0.5;
        }
        if let Some(s) = engine.atomic_skills.get_mut("search_memory") {
            s.proficiency = 0.5;
        }
        engine.auto_group_domains();
        assert!(engine.ability_domains.contains_key("domain-memory"));
    }

    #[test]
    fn test_create_domain() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test2.json"));
        engine.create_domain("memory-skills", "记忆能力域", &["remember_info", "search_memory"]);
        assert!(engine.ability_domains.contains_key("memory-skills"));
    }

    #[test]
    fn test_create_knowledge() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test3.json"));
        engine.create_knowledge("emotion-tone", "情绪影响语气", &["emotion", "core"], "injection");
        let kb = engine.knowledge_bodies.get("emotion-tone").unwrap();
        assert_eq!(kb.source, "injection");
    }

    #[test]
    fn test_auto_compose_knowledge_all() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test-compose.json"));
        engine.create_domain("emotion-skills", "情绪能力", &["detect_emotion"]);
        engine.create_domain("memory-skills", "记忆能力", &["remember_info"]);
        if let Some(d) = engine.ability_domains.get_mut("emotion-skills") {
            d.maturity = 0.7;
        }
        if let Some(d) = engine.ability_domains.get_mut("memory-skills") {
            d.maturity = 0.7;
        }
        engine.auto_compose_knowledge_all();
        assert!(engine.knowledge_bodies.contains_key("composed-emotion-skills-memory-skills"));
    }

    #[test]
    fn test_create_skill() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test-create.json"));
        assert!(engine.create_skill("new-skill", "新技能", "test", "描述", "正文").is_ok());
        assert!(engine.atomic_skills.contains_key("new-skill"));
    }

    #[test]
    fn test_suggest_practice() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test4.json"));
        if let Some(skill) = engine.atomic_skills.get_mut("understand_text") {
            skill.proficiency = 0.95;
        }
        let suggestions = engine.suggest_practice(3);
        assert!(!suggestions.is_empty());
        assert!(suggestions.iter().all(|(s, _)| s.id != "understand_text"));
    }

    #[test]
    fn test_learn_cycle() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test-cycle.json"));
        // 先练习几个技能提高熟练度
        engine.practice("remember_info", true, "记住神大人的喜好");
        engine.practice("search_memory", true, "搜索记忆");
        if let Some(s) = engine.atomic_skills.get_mut("remember_info") {
            s.proficiency = 0.5;
        }
        if let Some(s) = engine.atomic_skills.get_mut("search_memory") {
            s.proficiency = 0.5;
        }
        let stats = engine.learn_cycle();
        assert!(stats.total_domains >= 1);
    }
}
