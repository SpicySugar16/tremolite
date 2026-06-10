use std::any::Any;
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
    handle: Option<EngineHandle>,
    scheduler_tx: Option<mpsc::Sender<SessionTask>>,
}

#[derive(Clone)]
struct CronJobState {
    name: String,
    schedule: Schedule,
    action: JobAction,
    channel: String,
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

impl CronModule {
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(Mutex::new(Vec::new())),
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
            scheduler_tx: None,
        }
    }

    /// 设置调度器入站通道（由 Engine 在创建调度器后注入）
    pub fn set_scheduler(&mut self, tx: mpsc::Sender<SessionTask>) {
        self.scheduler_tx = Some(tx);
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

        thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_secs(5));
                let now = timestamp();
                let mut to_fire: Vec<(String, JobAction, String)> = Vec::new();

                if let Ok(mut jl) = jobs.lock() {
                    for job in jl.iter_mut() {
                        if !job.enabled || job.next_run > now {
                            continue;
                        }
                        to_fire.push((job.name.clone(), job.action.clone(), job.channel.clone()));
                        job.run_count += 1;
                        job.next_run = calc_next_run_at(&job.schedule, now);
                    }
                }

                for (name, action, channel) in to_fire {
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
                                    if !stdout.is_empty() {
                                        tracing::info!("cron: shell job '{}' stdout: {}", name, stdout.trim());
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
        vec![]
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
