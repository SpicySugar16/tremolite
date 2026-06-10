use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::fs;

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
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.use_count += 1;
        self.last_used = now;

        // 遗忘衰减：超过 24 小时没用，按时间衰减熟练度
        // 公式：proficiency *= exp(-闲置小时数 / (30天 * 24小时) * 衰减系数)
        // 衰减系数 2.0 —— 30 天完全遗忘
        let hours_idle = (now - self.last_used).saturating_sub(86400) as f64 / 3600.0;
        if hours_idle > 0.0 {
            let decay_factor = (-hours_idle / (30.0 * 24.0) * 2.0).exp();
            self.proficiency = (self.proficiency * decay_factor).max(0.05);
        }

        // 熟练度增长：用得越多越熟练，但增速递减
        self.proficiency = (self.proficiency + 0.05 * (1.0 - self.proficiency * 0.5)).min(1.0);

        // 成功率更新
        if success {
            self.success_rate = (self.success_rate * 0.9 + 1.0 * 0.1).min(1.0);
        } else {
            self.success_rate *= 0.95;
        }
    }

    /// 是否已掌握（熟练度 > 0.6）
    pub fn is_mastered(&self) -> bool {
        self.proficiency >= 0.6
    }

    /// 是否精通（熟练度 > 0.9）
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
    pub source: String,              // 来源：practice | composition | injection
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

// ─── 学习引擎核心 ─────────────────────────────

pub struct LearningEngine {
    atomic_skills: HashMap<String, AtomicSkill>,
    ability_domains: HashMap<String, AbilityDomain>,
    knowledge_bodies: HashMap<String, KnowledgeBody>,
    /// 练习日志——记录每次技能使用
    practice_log: VecDeque<PracticeRecord>,
    max_log_entries: usize,
    storage_path: PathBuf,
    dirty: bool,
}

/// 练习记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PracticeRecord {
    pub skill_id: String,
    pub success: bool,
    pub context: String,
    pub timestamp: u64,
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

        // 注入一些内置的原子技能
        let mut engine = Self {
            atomic_skills: skills,
            ability_domains: domains,
            knowledge_bodies: knowledge,
            practice_log: VecDeque::new(),
            max_log_entries: 500,
            storage_path,
            dirty: false,
        };
        engine.inject_builtin_skills();
        engine
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
                skill.proficiency = 0.3; // 初始就会一点
                skill
            });
        }
    }

    /// 练习一个技能
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
        Some(proficiency)
    }

    /// 获取技能
    pub fn get_skill(&self, id: &str) -> Option<&AtomicSkill> {
        self.atomic_skills.get(id)
    }

    /// 导出练习日志
    pub fn get_practice_log(&self) -> Vec<&PracticeRecord> {
        self.practice_log.iter().collect()
    }

    /// 创建能力域
    pub fn create_domain(&mut self, id: &str, name: &str, skill_ids: &[&str]) {
        let mut domain = AbilityDomain::new(id, name);
        for sid in skill_ids {
            domain.add_skill(sid);
        }
        domain.recalc_maturity(&self.atomic_skills);
        self.ability_domains.insert(id.to_string(), domain);
        self.dirty = true;
    }

    /// 创建知识体系
    pub fn create_knowledge(&mut self, id: &str, name: &str, domain_ids: &[&str], source: &str) {
        let mut knowledge = KnowledgeBody::new(id, name, source);
        for did in domain_ids {
            knowledge.domains.push(did.to_string());
        }
        self.knowledge_bodies.insert(id.to_string(), knowledge);
        self.dirty = true;
    }

    /// 验证知识
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

    /// 自动合成新知识——当两个域同时使用达到阈值时
    pub fn auto_compose(&mut self, domain_a: &str, domain_b: &str) -> Option<String> {
        let domain1 = self.ability_domains.get(domain_a)?;
        let domain2 = self.ability_domains.get(domain_b)?;

        // 两个域都足够成熟才能合成
        if domain1.maturity < 0.5 || domain2.maturity < 0.5 {
            return None;
        }

        let knowledge_id = format!("composed-{}-{}", domain_a, domain_b);
        let knowledge_name = format!("{}+{}", domain1.name, domain2.name);

        // 生成实际的描述内容——基于两个域包含的技能
        let skills_a: Vec<&str> = domain1.skills.iter().map(|s| s.as_str()).collect();
        let skills_b: Vec<&str> = domain2.skills.iter().map(|s| s.as_str()).collect();
        let description = format!(
            "交叉知识：当{}和{}同时使用时，透闪石能结合{}和{}中的技能进行跨域推理。",
            domain1.name, domain2.name,
            skills_a.join(", "),
            skills_b.join(", ")
        );

        self.create_knowledge(
            &knowledge_id,
            &knowledge_name,
            &[domain_a, domain_b],
            "composition",
        );

        // 填充描述
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
            .filter(|s| !s.is_expert()) // 还没精通的
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

    /// 获取某项技能的成功率（没有数据则返回 0.5——不确定）
    pub fn get_success_rate(&self, skill_id: &str) -> f64 {
        self.atomic_skills
            .get(skill_id)
            .map(|s| s.success_rate)
            .unwrap_or(0.5)
    }

    /// 获取某项技能的熟练度（没有数据则返回 0.0）
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

        SearchResult {
            skills,
            domains,
            knowledge,
        }
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
    fn test_suggest_practice() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test4.json"));
        // 让一个技能熟练度高
        if let Some(skill) = engine.atomic_skills.get_mut("understand_text") {
            skill.proficiency = 0.95;
        }
        let suggestions = engine.suggest_practice(3);
        assert!(!suggestions.is_empty());
        // 高熟练度的不会在推荐里
        assert!(suggestions.iter().all(|(s, _)| s.id != "understand_text"));
    }

    #[test]
    fn test_auto_compose() {
        let mut engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test5.json"));
        engine.create_domain("emotion-skills", "情绪能力", &["detect_emotion"]);
        engine.create_domain("memory-skills", "记忆能力", &["remember_info"]);

        // 手动提高成熟度
        if let Some(domain) = engine.ability_domains.get_mut("emotion-skills") {
            domain.maturity = 0.7;
        }
        if let Some(domain) = engine.ability_domains.get_mut("memory-skills") {
            domain.maturity = 0.7;
        }

        let result = engine.auto_compose("emotion-skills", "memory-skills");
        assert!(result.is_some());
    }

    #[test]
    fn test_search() {
        let engine = LearningEngine::new(PathBuf::from("/tmp/tremolite-learn-test6.json"));
        let result = engine.search("情绪");
        assert!(!result.skills.is_empty() || !result.domains.is_empty());
    }
}
