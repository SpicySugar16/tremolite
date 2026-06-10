use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::fs;

// ─── 计划书系统的核心数据 ──────────────────────

/// 计划状态——生命周期
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum PlanStatus {
    /// 草稿——刚创建，还在构思
    Draft,
    /// 审核中——等待确认
    Reviewing,
    /// 已批准——可以开始执行
    Approved,
    /// 进行中——正在执行
    InProgress,
    /// 已完成——所有步骤完成
    Completed,
    /// 已取消——中途放弃或合并
    Cancelled,
    /// 已归档——历史记录
    Archived,
}

impl PlanStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PlanStatus::Draft => "draft",
            PlanStatus::Reviewing => "reviewing",
            PlanStatus::Approved => "approved",
            PlanStatus::InProgress => "in progress",
            PlanStatus::Completed => "completed",
            PlanStatus::Cancelled => "cancelled",
            PlanStatus::Archived => "archived",
        }
    }

    /// 是否可以推进到下一个合理状态
    pub fn can_transition_to(&self, target: PlanStatus) -> bool {
        match (self, target) {
            (PlanStatus::Draft, PlanStatus::Reviewing)
            | (PlanStatus::Draft, PlanStatus::Cancelled) => true,
            (PlanStatus::Reviewing, PlanStatus::Approved)
            | (PlanStatus::Reviewing, PlanStatus::Draft)
            | (PlanStatus::Reviewing, PlanStatus::Cancelled) => true,
            (PlanStatus::Approved, PlanStatus::InProgress)
            | (PlanStatus::Approved, PlanStatus::Cancelled) => true,
            (PlanStatus::InProgress, PlanStatus::Completed)
            | (PlanStatus::InProgress, PlanStatus::Draft)
            | (PlanStatus::InProgress, PlanStatus::Cancelled) => true,
            (PlanStatus::Completed, PlanStatus::Archived) => true,
            (PlanStatus::Cancelled, PlanStatus::Archived) => true,
            (PlanStatus::Archived, PlanStatus::Draft) => true, // 可以重新激活
            _ => false,
        }
    }
}

/// 步骤的状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
    Skipped,
}

impl StepStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StepStatus::Pending => "pending",
            StepStatus::InProgress => "in progress",
            StepStatus::Completed => "completed",
            StepStatus::Blocked => "blocked",
            StepStatus::Skipped => "skipped",
        }
    }
}

/// 计划书中的一个步骤
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub status: StepStatus,
    pub depends_on: Vec<u64>,   // 前置步骤 id
    pub assigned_to: String,    // 谁做
    pub estimated_effort: u32,  // 预估分钟数
    pub actual_effort: u32,     // 实际分钟数
    pub created_at: u64,
    pub completed_at: Option<u64>,
    pub notes: String,          // 执行备注
}

impl PlanStep {
    pub fn new(id: u64, title: &str, description: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            title: title.to_string(),
            description: description.to_string(),
            status: StepStatus::Pending,
            depends_on: Vec::new(),
            assigned_to: "aoi".into(),
            estimated_effort: 30,
            actual_effort: 0,
            created_at: now,
            completed_at: None,
            notes: String::new(),
        }
    }

    /// 是否可执行（所有依赖已完成）
    pub fn is_executable(&self, steps: &[PlanStep]) -> bool {
        if self.status != StepStatus::Pending && self.status != StepStatus::Blocked {
            return false;
        }
        self.depends_on.iter().all(|dep_id| {
            steps.iter().any(|s| s.id == *dep_id && s.status == StepStatus::Completed)
        })
    }
}

/// 完整的计划书
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: u64,
    pub title: String,
    pub description: String,
    pub status: PlanStatus,
    pub steps: Vec<PlanStep>,
    pub tags: Vec<String>,
    pub created_at: u64,
    pub updated_at: u64,
    pub completed_at: Option<u64>,
    pub source: String,          // 来自哪个对话/事件
    pub priority: u8,            // 1~5，5最高
}

impl Plan {
    pub fn new(id: u64, title: &str, description: &str) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        Self {
            id,
            title: title.to_string(),
            description: description.to_string(),
            status: PlanStatus::Draft,
            steps: Vec::new(),
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            completed_at: None,
            source: String::new(),
            priority: 3,
        }
    }

    /// 添加一个步骤
    pub fn add_step(&mut self, step: PlanStep) {
        self.steps.push(step);
        self.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
    }

    /// 获取可执行的下一步骤
    pub fn next_executable_step(&self) -> Option<&PlanStep> {
        let mut executable: Vec<&PlanStep> = self
            .steps
            .iter()
            .filter(|s| s.is_executable(&self.steps))
            .collect();
        executable.sort_by_key(|s| s.id);
        executable.into_iter().next()
    }

    /// 完成进度百分比
    pub fn progress(&self) -> f64 {
        if self.steps.is_empty() {
            if self.status == PlanStatus::Completed {
                return 1.0;
            }
            return 0.0;
        }
        let completed = self
            .steps
            .iter()
            .filter(|s| s.status == StepStatus::Completed)
            .count() as f64;
        completed / self.steps.len() as f64
    }
}

// ─── 计划书管理器 ──────────────────────────────

/// 计划书管理器——透闪石的计划书系统核心
pub struct PlanManager {
    plans: Vec<Plan>,
    storage_path: PathBuf,
    next_id: u64,
    dirty: bool,
}

impl PlanManager {
    pub fn new(storage_path: PathBuf) -> Self {
        let plans = if storage_path.exists() {
            fs::read_to_string(&storage_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        let next_id = plans.iter().map(|p: &Plan| p.id).max().unwrap_or(0) + 1;

        Self {
            plans,
            storage_path,
            next_id,
            dirty: false,
        }
    }

    /// 创建新计划书
    pub fn create_plan(&mut self, title: &str, description: &str) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let plan = Plan::new(id, title, description);
        self.plans.push(plan);
        self.dirty = true;
        id
    }

    /// 获取计划书
    pub fn get_plan(&self, id: u64) -> Option<&Plan> {
        self.plans.iter().find(|p| p.id == id)
    }

    /// 可变引用
    pub fn get_plan_mut(&mut self, id: u64) -> Option<&mut Plan> {
        self.dirty = true;
        self.plans.iter_mut().find(|p| p.id == id)
    }

    /// 按状态过滤
    pub fn filter_by_status(&self, status: PlanStatus) -> Vec<&Plan> {
        self.plans.iter().filter(|p| p.status == status).collect()
    }

    /// 搜索计划书
    pub fn search(&self, keyword: &str) -> Vec<&Plan> {
        let lower = keyword.to_lowercase();
        self.plans
            .iter()
            .filter(|p| {
                p.title.to_lowercase().contains(&lower)
                    || p.description.to_lowercase().contains(&lower)
                    || p.tags.iter().any(|t| t.contains(&lower))
                    || p
                        .steps
                        .iter()
                        .any(|s| s.title.to_lowercase().contains(&lower))
            })
            .collect()
    }

    /// 更新计划书状态
    pub fn transition_status(&mut self, plan_id: u64, target: PlanStatus) -> Result<(), String> {
        let plan = self
            .plans
            .iter_mut()
            .find(|p| p.id == plan_id)
            .ok_or_else(|| format!("plan {} not found", plan_id))?;

        if !plan.status.can_transition_to(target) {
            return Err(format!(
                "cannot transition from {:?} to {:?}",
                plan.status, target
            ));
        }

        plan.status = target;
        plan.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if target == PlanStatus::Completed {
            plan.completed_at = Some(plan.updated_at);
        }

        self.dirty = true;
        Ok(())
    }

    /// 标记一个步骤为完成，并自动推进依赖解锁和计划完成
    pub fn complete_step(
        &mut self,
        plan_id: u64,
        step_id: u64,
    ) -> Result<AutoAdvanceResult, String> {
        let plan = self
            .plans
            .iter_mut()
            .find(|p| p.id == plan_id)
            .ok_or_else(|| format!("plan {} not found", plan_id))?;

        let step = plan
            .steps
            .iter_mut()
            .find(|s| s.id == step_id)
            .ok_or_else(|| format!("step {} not found", step_id))?;

        step.status = StepStatus::Completed;
        step.completed_at = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        );

        self.dirty = true;

        // 自动推进：收集需要解锁和新完成数
        let completed_ids: Vec<u64> = plan.steps.iter()
            .filter(|s| s.status == StepStatus::Completed)
            .map(|s| s.id)
            .collect();

        let mut to_unblock: Vec<u64> = Vec::new();
        // 不可变遍历收集需要解锁的步骤
        for s in plan.steps.iter() {
            if s.status == StepStatus::Blocked {
                let all_deps_done = s.depends_on.iter().all(|dep_id| {
                    completed_ids.contains(dep_id)
                });
                if all_deps_done && !s.depends_on.is_empty() {
                    to_unblock.push(s.id);
                }
            }
        }

        // 第二遍：执行解锁
        for s in plan.steps.iter_mut() {
            if to_unblock.contains(&s.id) {
                s.status = StepStatus::Pending;
            }
        }

        // 检查是否所有步骤都完成
        let all_done = plan.steps.iter().all(|s| {
            s.status == StepStatus::Completed || s.status == StepStatus::Skipped
        });
        let plan_completed = if all_done && !plan.steps.is_empty() && plan.status != PlanStatus::Completed {
            plan.status = PlanStatus::Completed;
            plan.completed_at = Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            plan.updated_at = plan.completed_at.unwrap();
            self.dirty = true;
            true
        } else {
            false
        };

        Ok(AutoAdvanceResult {
            unblocked: to_unblock,
            plan_completed,
        })
    }

    /// 生成操作手册（checklist格式）
    pub fn generate_handbook(&self, plan_id: u64) -> Result<String, String> {
        let plan = self
            .plans
            .iter()
            .find(|p| p.id == plan_id)
            .ok_or_else(|| format!("plan {} not found", plan_id))?;

        let mut handbook = String::new();
        handbook.push_str(&format!("# {} - {}\n\n", plan.title, plan.description));
        handbook.push_str(&format!(
            "Status: {} | Priority: {} | Progress: {:.0}%\n\n",
            plan.status.as_str(),
            plan.priority,
            plan.progress() * 100.0
        ));

        handbook.push_str("## Steps\n\n");
        for step in &plan.steps {
            let mark = match step.status {
                StepStatus::Completed => "[x]",
                StepStatus::InProgress => "[-]",
                StepStatus::Blocked => "[!]",
                StepStatus::Skipped => "[~]",
                StepStatus::Pending => "[ ]",
            };
            let dep_info = if step.depends_on.is_empty() {
                String::new()
            } else {
                format!(" (depends on: {:?})", step.depends_on)
            };
            handbook.push_str(&format!(
                "- {} **{}**: {}{}\n",
                mark, step.title, step.description, dep_info
            ));
            if !step.notes.is_empty() {
                handbook.push_str(&format!("  - note: {}\n", step.notes));
            }
        }

        Ok(handbook)
    }

    /// 保存到磁盘
    pub fn flush(&mut self) -> Result<(), String> {
        if !self.dirty {
            return Ok(());
        }
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&self.plans).map_err(|e| e.to_string())?;
        fs::write(&self.storage_path, json).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }

    /// 统计
    pub fn stats(&self) -> PlanStats {
        PlanStats {
            total: self.plans.len(),
            draft: self.filter_by_status(PlanStatus::Draft).len(),
            in_progress: self.filter_by_status(PlanStatus::InProgress).len(),
            completed: self.filter_by_status(PlanStatus::Completed).len(),
            total_steps: self.plans.iter().map(|p| p.steps.len()).sum(),
        }
    }
}

impl Drop for PlanManager {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

/// 自动推进的结果——complete_step 返回的附加信息
#[derive(Debug, Clone, Serialize)]
pub struct AutoAdvanceResult {
    /// 被解锁的步骤 ID 列表
    pub unblocked: Vec<u64>,
    /// 计划是否因所有步骤完成而自动完结
    pub plan_completed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlanStats {
    pub total: usize,
    pub draft: usize,
    pub in_progress: usize,
    pub completed: usize,
    pub total_steps: usize,
}

// ─── 单元测试 ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_lifecycle() {
        let mut pm = PlanManager::new(PathBuf::from("/tmp/tremolite-plan-test.json"));
        let id = pm.create_plan("测试透闪石", "测试计划书生命周期");
        assert!(pm.transition_status(id, PlanStatus::Reviewing).is_ok());
        assert!(pm.transition_status(id, PlanStatus::Approved).is_ok());
        assert!(pm.transition_status(id, PlanStatus::InProgress).is_ok());
        assert!(pm.transition_status(id, PlanStatus::Completed).is_ok());
        // 不能从已完成跳回草稿
        assert!(pm.transition_status(id, PlanStatus::Draft).is_err());
    }

    #[test]
    fn test_plan_steps() {
        let mut pm = PlanManager::new(PathBuf::from("/tmp/tremolite-plan-test2.json"));
        let id = pm.create_plan("搭建记忆系统", "完成五层缓存记忆");
        let plan = pm.get_plan_mut(id).unwrap();
        let step1 = PlanStep::new(1, "设计数据结构", "定义MemoryEntry和层级");
        let step2 = PlanStep::new(2, "实现L1", "工作记忆LRU缓存");
        plan.add_step(step1);
        plan.add_step(step2);

        plan.steps[0].status = StepStatus::Completed;
        assert!(plan.next_executable_step().is_some());
    }

    #[test]
    fn test_generate_handbook() {
        let mut pm = PlanManager::new(PathBuf::from("/tmp/tremolite-plan-test3.json"));
        let id = pm.create_plan("情绪引擎", "八维情绪向量");
        {
            let plan = pm.get_plan_mut(id).unwrap();
            plan.add_step(PlanStep::new(1, "EmotionState", "定义八维结构"));
            plan.add_step(PlanStep::new(2, "关键词检测", "实现detect_from_text"));
            plan.steps[0].status = StepStatus::Completed;
        }
        let _ = pm.transition_status(id, PlanStatus::InProgress);
        let handbook = pm.generate_handbook(id).unwrap();
        assert!(handbook.contains("情绪引擎"));
        assert!(handbook.contains("[x]"));
        assert!(handbook.contains("[ ]"));
    }

    #[test]
    fn test_search() {
        let path = std::env::temp_dir().join("tremolite-plan-test4.json");
        let _ = std::fs::remove_file(&path);
        let mut pm = PlanManager::new(path);
        pm.create_plan("情绪引擎", "八维情绪向量检测");
        pm.create_plan("记忆系统", "五层缓存记忆");
        let results = pm.search("情绪");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_progress() {
        let mut pm = PlanManager::new(PathBuf::from("/tmp/tremolite-plan-test5.json"));
        let id = pm.create_plan("测试进度", "测试进度计算");
        {
            let plan = pm.get_plan_mut(id).unwrap();
            plan.add_step(PlanStep::new(1, "step1", ""));
            plan.add_step(PlanStep::new(2, "step2", ""));
            plan.add_step(PlanStep::new(3, "step3", ""));
            plan.steps[0].status = StepStatus::Completed;
            plan.steps[1].status = StepStatus::InProgress;
        }
        let plan = pm.get_plan(id).unwrap();
        assert!((plan.progress() - 0.333).abs() < 0.01);
    }
}
