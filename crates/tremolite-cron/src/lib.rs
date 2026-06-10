use serde::{Deserialize, Serialize};

// ─── 调度计划 ──────────────────────────────────────

/// 单次或循环任务
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Schedule {
    /// 每 N 秒执行一次
    #[serde(rename = "every")]
    EverySecs(u64),
    /// 每天的特定 UTC 小时:分钟执行
    #[serde(rename = "daily")]
    Daily { hour: u8, minute: u8 },
    /// 仅执行一次（延迟 N 秒后）
    #[serde(rename = "once")]
    Once { delay_secs: u64 },
    /// cron 表达式格式: "分 时 日 月 周" (5字段)
    #[serde(rename = "cron")]
    CronExpr(String),
}

// ─── 执行动作类型 ──────────────────────────────────

/// Cron job 触发的动作
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CronAction {
    /// 执行 shell 命令
    #[serde(rename = "shell")]
    Shell {
        /// shell 命令
        command: String,
    },
    /// 通过 LLM 处理 prompt
    #[serde(rename = "prompt")]
    LlmPrompt {
        /// 发给 LLM 的 prompt
        prompt: String,
    },
}

// ─── 对外的条目信息 ────────────────────────────────

/// Cron 任务对外信息（无内部状态）
#[derive(Debug, Clone, Serialize)]
pub struct CronEntryInfo {
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    pub channel: String,
    pub next_run: u64,
    pub run_count: u64,
    pub enabled: bool,
}

// ─── 配置反序列化 ──────────────────────────────────

/// 用于从 config.toml 反序列化的 cron job 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobConfig {
    /// job 名称（可选，默认使用 config key）
    #[serde(default)]
    pub name: Option<String>,
    /// 调度计划
    pub schedule: Schedule,
    /// 执行动作
    pub action: CronAction,
    /// 是否默认启用（默认 true）
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

// ─── 工具函数 ──────────────────────────────────────

/// 计算下次执行时间（基于当前时间）
pub fn calc_next_run(schedule: &Schedule) -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    calc_next_run_at(schedule, now)
}

/// 计算基于指定时间戳的下次执行时间
pub fn calc_next_run_at(schedule: &Schedule, now: u64) -> u64 {
    match schedule {
        Schedule::EverySecs(secs) => now + secs,
        Schedule::Once { delay_secs } => now + delay_secs,
        Schedule::Daily { hour, minute } => {
            let target = *hour as u64 * 3600 + *minute as u64 * 60;
            let today_secs = now % 86400;
            if today_secs < target {
                now - today_secs + target
            } else {
                now - today_secs + 86400 + target
            }
        }
        Schedule::CronExpr(_) => now + 60,
    }
}
