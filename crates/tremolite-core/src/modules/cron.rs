use std::any::Any;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use tremolite_cron::{CronEntryInfo, Schedule, calc_next_run_at};
use tremolite_llm::{ToolDefinition, ToolFunction};

use crate::module::{
    Capability, EngineHandle, Event, EventContext, EventResponse, Module, ModuleError,
};
use crate::scheduler::SessionTask;

/// Cron 模块——独立的定时任务调度器
///
/// 在后台线程中运行，每 5 秒 tick 一次。
/// 到期任务通过调度器的 inbound 通道投递 SessionTask。
pub struct CronModule {
    jobs: Arc<Mutex<Vec<CronJobState>>>,
    running: Arc<AtomicBool>,
    #[allow(dead_code)]
    handle: Option<EngineHandle>,
    scheduler_tx: Option<mpsc::Sender<SessionTask>>,
    /// cron_tasks.json 路径——ticker 每 tick 从这里重载任务
    json_path: Option<PathBuf>,
}

#[derive(Clone)]
struct CronJobState {
    name: String,
    schedule: Schedule,
    action: JobAction,
    channel: String,
    /// 可选的任务级投递目标，如 "group:123456" 或 "private:654321"
    deliver_target: Option<String>,
    next_run: u64,
    run_count: u64,
    enabled: bool,
}

#[derive(Clone)]
enum JobAction {
    Prompt(String),
    Shell(String),
}

fn timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 把 "0 30 8 * * *" 或 "0 */30 * * * *" 这类 cron 字符串转成 Schedule
fn parse_schedule_str(s: &str) -> Schedule {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 5 {
        // 间隔模式: "0 */N * * * *"
        if parts.len() >= 6 && parts[0] == "0" && parts[1].starts_with("*/") {
            let n: u64 = parts[1].trim_start_matches("*/").parse().unwrap_or(30);
            return Schedule::EverySecs(n * 60);
        }
        if parts.len() >= 6 && parts[0] == "0" && parts[1] == "0" && parts[2].starts_with("*/") {
            let n: u64 = parts[2].trim_start_matches("*/").parse().unwrap_or(1);
            return Schedule::EverySecs(n * 3600);
        }
        // 定时模式: "0 MM HH * * *" -> Daily { hour, minute }
        if parts.len() >= 6 && parts[3] == "*" && parts[4] == "*" && parts[5] == "*" {
            let hour: u8 = parts[2].parse().unwrap_or(0);
            let minute: u8 = parts[1].parse().unwrap_or(0);
            return Schedule::Daily { hour, minute };
        }
    }
    // 兜底：原始字符串作为 cron 表达式（截 5 字段）
    let five: String = parts.iter().take(5).cloned().collect::<Vec<_>>().join(" ");
    Schedule::CronExpr(five)
}

/// 把 "all" "broadcast" 等归一化为 __all__，否则原样
/// 注意：处理 "channel:type:id" 格式时只取通道名
fn normalize_deliver(d: &str) -> &str {
    match d {
        "" | "all" | "broadcast" | "everywhere" => "__all__",
        _ => {
            // "channel:type:id" → 提取 "channel" 部分
            if let Some(pos) = d.find(':') {
                &d[..pos]
            } else {
                d
            }
        }
    }
}

/// 从 "channel:type:id" 格式中提取 ":type:id" 部分作为投递目标
fn extract_deliver_target(d: &str) -> Option<String> {
    let pos = d.find(':')?;
    let rest = &d[pos + 1..];
    if rest.is_empty() { None } else { Some(rest.to_string()) }
}

impl CronModule {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            scheduler_tx: None,
            json_path: None,
        }
    }

    /// 设置 cron_tasks.json 路径
    pub fn set_json_path(&mut self, path: &str) {
        self.json_path = Some(PathBuf::from(path));
    }

    /// 设置调度器入站通道（由 Engine 在创建调度器后注入）
    pub fn set_scheduler(&mut self, tx: mpsc::Sender<SessionTask>) {
        self.scheduler_tx = Some(tx);
    }

    /// 从 cron_tasks.json 重载全部任务
    pub fn load_from_json(&self) {
        let path = match &self.json_path {
            Some(p) => p.clone(),
            None => return,
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return,
        };
        let entries: Vec<serde_json::Value> = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return,
        };
        let mut jobs = self.jobs.lock().unwrap();
        jobs.clear();
        let now = timestamp();
        for entry in &entries {
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("未命名");
            let sched_str = entry.get("schedule").and_then(|v| v.as_str()).unwrap_or("");
            let sched = parse_schedule_str(sched_str);
            let cmd = entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let channel_raw = entry.get("deliver").and_then(|v| v.as_str()).unwrap_or("origin");
            let channel = normalize_deliver(channel_raw);
            let deliver_target = extract_deliver_target(channel_raw);
            let enabled = entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
            let next_run = entry.get("next_run").and_then(|v| v.as_u64()).unwrap_or_else(|| calc_next_run_at(&sched, now));
            let action = if entry.get("type").and_then(|v| v.as_str()) == Some("prompt") {
                JobAction::Prompt(cmd.to_string())
            } else {
                JobAction::Shell(cmd.to_string())
            };
            jobs.push(CronJobState {
                name: name.to_string(),
                schedule: sched,
                action,
                channel: channel.to_string(),
                deliver_target,
                next_run,
                run_count: 0,
                enabled,
            });
        }
        tracing::info!("cron: loaded {} tasks from json", jobs.len());
    }

    /// 注册一个定时 prompt 任务
    pub fn add_job(&mut self, name: &str, schedule: Schedule, prompt: &str, channel: &str) {
        let now = timestamp();
        let next_run = calc_next_run_at(&schedule, now);
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(CronJobState {
            name: name.to_string(),
            schedule,
            action: JobAction::Prompt(prompt.to_string()),
            channel: channel.to_string(),
            deliver_target: None,
            next_run,
            run_count: 0,
            enabled: true,
        });
        tracing::info!("cron: registered prompt job '{}'", name);
    }

    /// 注册一个定时 shell 任务
    pub fn add_shell_job(&mut self, name: &str, schedule: Schedule, command: &str, channel: &str) {
        let now = timestamp();
        let next_run = calc_next_run_at(&schedule, now);
        let mut jobs = self.jobs.lock().unwrap();
        jobs.push(CronJobState {
            name: name.to_string(),
            schedule,
            action: JobAction::Shell(command.to_string()),
            channel: channel.to_string(),
            deliver_target: None,
            next_run,
            run_count: 0,
            enabled: true,
        });
        tracing::info!("cron: registered shell job '{}'", name);
    }

    /// 列出所有任务
    pub fn list_jobs(&self) -> Vec<CronEntryInfo> {
        let jobs = self.jobs.lock().unwrap();
        jobs.iter()
            .map(|j| {
                let action_desc = match &j.action {
                    JobAction::Prompt(p) => format!("prompt: {}", p.chars().take(40).collect::<String>()),
                    JobAction::Shell(c) => format!("shell: {}", c.chars().take(40).collect::<String>()),
                };
                CronEntryInfo {
                    name: j.name.clone(),
                    schedule: format!("{:?}", j.schedule),
                    prompt: action_desc,
                    channel: j.channel.clone(),
                    next_run: j.next_run,
                    run_count: j.run_count,
                    enabled: j.enabled,
                }
            })
            .collect()
    }

    fn spawn_ticker(&mut self) {
        self.running.store(true, Ordering::Relaxed);
        let running = self.running.clone();
        let jobs = self.jobs.clone();
        let tx = match &self.scheduler_tx {
            Some(t) => t.clone(),
            None => return,
        };

        // 挂 json 路径的拷贝
        let json_path = self.json_path.clone();

        thread::spawn(move || {
            let json_path = json_path;
            while running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(5));

                // 每 tick 从 json 重载——确保 API 新建的任务被拾取
                if let Some(ref path) = json_path {
                    let content = std::fs::read_to_string(path).ok();
                    if let Some(c) = content {
                        if let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(&c) {
                            if let Ok(mut jl) = jobs.lock() {
                                jl.clear();
                                let now = timestamp();
                                for entry in &entries {
                                    let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("未命名");
                                    let sched_str = entry.get("schedule").and_then(|v| v.as_str()).unwrap_or("");
                                    let sched = parse_schedule_str(sched_str);
                                    let cmd = entry.get("command").and_then(|v| v.as_str()).unwrap_or("");
                                    let channel_raw = entry.get("deliver").and_then(|v| v.as_str()).unwrap_or("origin");
                                    let channel = normalize_deliver(channel_raw);
                                    let deliver_target = extract_deliver_target(channel_raw);
                                    let enabled = entry.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                                    let next_run = entry.get("next_run").and_then(|v| v.as_u64()).unwrap_or_else(|| calc_next_run_at(&sched, now));
                                    let action = if entry.get("type").and_then(|v| v.as_str()) == Some("prompt") {
                                        JobAction::Prompt(cmd.to_string())
                                    } else {
                                        JobAction::Shell(cmd.to_string())
                                    };
                                    jl.push(CronJobState {
                                        name: name.to_string(),
                                        schedule: sched,
                                        action,
                                        channel: channel.to_string(),
                                        deliver_target,
                                        next_run,
                                        run_count: 0,
                                        enabled,
                                    });
                                }
                            }
                        }
                    }
                }

                let now = timestamp();
                let mut to_fire: Vec<(String, JobAction, String, String, u64)> = Vec::new();

                if let Ok(mut jl) = jobs.lock() {
                    for job in jl.iter_mut() {
                        if !job.enabled || job.next_run > now {
                            continue;
                        }
                        let sender = match &job.deliver_target {
                            Some(t) => t.clone(),
                            None => format!("cron-{}", job.name),
                        };
                        job.next_run = calc_next_run_at(&job.schedule, now);
                        let next = job.next_run;
                        to_fire.push((job.name.clone(), job.action.clone(), job.channel.clone(), sender, next));
                        job.run_count += 1;
                    }
                }

                for (name, action, channel, sender, next) in to_fire {
                    match action {
                        JobAction::Prompt(prompt) => {
                            let _ = tx.send(SessionTask {
                                session_id: format!("cron-{}", name),
                                input: prompt,
                                channel,
                                sender: format!("cron-{}", name),
                            });
                            tracing::info!("cron: prompt job '{}' fired", name);
                        }
                        JobAction::Shell(command) => {
                            tracing::info!("cron: shell job '{}' executing: {}", name, command);
                            let output = std::process::Command::new("sh")
                                .arg("-c")
                                .arg(&command)
                                .output();
                            match output {
                                Ok(out) => {
                                    let stdout = String::from_utf8_lossy(&out.stdout);
                                    let stderr = String::from_utf8_lossy(&out.stderr);
                                    if !stdout.trim().is_empty() {
                                        tracing::info!("cron: shell job '{}' stdout: {}", name, stdout.trim());
                                        let _ = tx.send(SessionTask {
                                            session_id: format!("cron-{}", name),
                                            input: stdout.trim().to_string(),
                                            channel: channel.clone(),
                                            sender: sender.clone(),
                                        });
                                    }
                                    if !stderr.is_empty() {
                                        tracing::warn!("cron: shell job '{}' stderr: {}", name, stderr.trim());
                                    }
                                    tracing::info!("cron: shell job '{}' exited with {}", name, out.status);
                                }
                                Err(e) => {
                                    tracing::error!("cron: shell job '{}' failed to execute: {}", name, e);
                                }
                            }
                        }
                    }

                    if let Some(ref path) = json_path {
                        if let Ok(content) = std::fs::read_to_string(path) {
                            if let Ok(mut entries) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                                for entry in &mut entries {
                                    if entry.get("name").and_then(|v| v.as_str()) == Some(&name) {
                                        if let Some(obj) = entry.as_object_mut() {
                                            obj.insert("last_run".into(), serde_json::json!(now));
                                            obj.insert("next_run".into(), serde_json::json!(next));
                                        }
                                    }
                                }
                                let _ = std::fs::write(path, serde_json::to_string_pretty(&entries).unwrap_or_default());
                            }
                        }
                    }
                }
            }
        });
    }
}

impl Module for CronModule {
    fn id(&self) -> &str {
        "cron"
    }
    fn name(&self) -> &str {
        "定时任务"
    }
    fn version(&self) -> &str {
        "0.3.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec!["cron.schedule".into(), "cron.list".into()]
    }
    fn requires(&self) -> Vec<Capability> {
        vec!["channels.qqbot".into(), "channels.napcat".into(), "channels.http".into()]
    }

    fn required_modules(&self) -> Vec<&str> {
        vec!["channels"]
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            def_type: "function".into(),
            function: ToolFunction {
                name: "cron_list".into(),
                description: "列出所有已注册的定时任务".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
            },
        }]
    }

    fn execute_tool(&mut self, name: &str, _args: &str) -> Result<String, ModuleError> {
        match name {
            "cron_list" => {
                let info = self.list_jobs();
                if info.is_empty() {
                    Ok("暂无定时任务呢~".into())
                } else {
                    let mut out = format!("定时任务（{}）：", info.len());
                    for j in &info {
                        let status = if j.enabled { "🟢" } else { "🔴" };
                        out.push_str(&format!(
                            "\n{} {} — {} (下次: {}秒后, 已跑 {} 次)",
                            status,
                            j.name,
                            j.schedule,
                            j.next_run.saturating_sub(timestamp()),
                            j.run_count
                        ));
                    }
                    Ok(out)
                }
            }
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn on_event(
        &mut self,
        event: &Event,
        _ctx: &EventContext,
    ) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                self.spawn_ticker();
                tracing::info!("cron: module ready");
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                self.running.store(false, Ordering::Relaxed);
                tracing::info!("cron: module stopped");
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
