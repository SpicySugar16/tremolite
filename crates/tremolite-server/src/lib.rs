use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc, atomic::{AtomicU64, Ordering}};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::{Extension, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::{Html, Json, IntoResponse},
    routing::{get, post},
    http::{header, HeaderMap, HeaderValue},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tower_http::cors::CorsLayer;

use tremolite_core::scheduler::SessionTask;
use tremolite_tools::{ToolRegistry, register_all};
use tremolite_cron::Schedule;
use tremolite_cron::VERSION as CRON_VERSION;
use tremolite_emotion::VERSION as EMOTION_VERSION;
use tremolite_tools::VERSION as TOOLS_VERSION;
use tremolite_channels::VERSION as CHANNELS_VERSION;
use tremolite_learn::VERSION as LEARN_VERSION;
use tremolite_memory::VERSION as MEMORY_VERSION;
use tremolite_attention::VERSION as ATTENTION_VERSION;
use tremolite_compress::VERSION as COMPRESS_VERSION;
use tremolite_reflection::VERSION as REFLECTION_VERSION;
use tremolite_delegation::VERSION as DELEGATION_VERSION;
use tremolite_mcp::VERSION as MCP_VERSION;
use tremolite_session::VERSION as SESSION_VERSION;
use tremolite_plan::VERSION as PLAN_VERSION;
use tremolite_dashboard::dashboard_html;
use tremolite_message::ChannelRegistry;

pub mod prompts;

// ─── 共享状态（HTTP 端使用调度器，不持有引擎锁） ────

struct AppState {
    /// 调度器入站发送端——所有消息统一投此通道
    inbound_tx: mpsc::Sender<SessionTask>,
    /// 待返回结果表——HTTP handler 等同步等回复用
    pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    // 原始启动时间
    started_at: Duration,
    // 性能指标
    total_requests: AtomicU64,
    active_connections: AtomicU64,
    // 当前配置包名称
    profile_name: String,
    // 通道注册表（共享引用，用于查询已知目标）
    #[allow(dead_code)]
    channel_registry: Option<Arc<tokio::sync::Mutex<ChannelRegistry>>>,
    // 模块注册表——用于访问引擎内模块
    #[allow(dead_code)]
    modules: Option<tremolite_core::module::ModuleRegistry>,
}

impl AppState {
    fn uptime_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(self.started_at.as_secs())
    }
}

// ─── 全局指标收集器 ─────────────────────────────

pub struct Metrics {
    pub total_requests: AtomicU64,
    pub total_errors: AtomicU64,
    pub total_tool_calls: AtomicU64,
    pub total_llm_calls: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            total_errors: AtomicU64::new(0),
            total_tool_calls: AtomicU64::new(0),
            total_llm_calls: AtomicU64::new(0),
        }
    }
}

pub static GLOBAL_METRICS: Metrics = Metrics {
    total_requests: AtomicU64::new(0),
    total_errors: AtomicU64::new(0),
    total_tool_calls: AtomicU64::new(0),
    total_llm_calls: AtomicU64::new(0),
};

// ─── 启动函数 ─────────────────────────────────────

pub async fn run_server(
    inbound_tx: mpsc::Sender<SessionTask>,
    pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    addr: &str,
    profile_name: &str,
    channel_registry: Option<Arc<tokio::sync::Mutex<ChannelRegistry>>>,
    modules: Option<tremolite_core::module::ModuleRegistry>,
) -> Result<(), String> {
    run_server_inner(inbound_tx, pending_results, addr, profile_name, channel_registry, modules).await
}

async fn run_server_inner(
    inbound_tx: mpsc::Sender<SessionTask>,
    pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    addr: &str,
    profile_name: &str,
    channel_registry: Option<Arc<tokio::sync::Mutex<ChannelRegistry>>>,
    modules: Option<tremolite_core::module::ModuleRegistry>,
) -> Result<(), String> {
    let state = Arc::new(AppState {
        inbound_tx,
        pending_results,
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
        total_requests: AtomicU64::new(0),
        active_connections: AtomicU64::new(0),
        profile_name: profile_name.to_string(),
        channel_registry,
        modules,
    });

    let router = Router::new()
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
        .route("/chat", post(handle_chat))
        .route("/webhooks/{name}", post(handle_webhook))
        .route("/ws", get(handle_ws))
        .route("/dashboard", get(handle_dashboard))
        .route("/dashboard/status", get(handle_dashboard_status))
        .route("/dashboard/profiles", get(handle_profile_list))
        .route("/dashboard/profiles/load", post(handle_profile_load))
        .route("/dashboard/config", get(handle_dashboard_config))
        .route("/dashboard/logs", get(handle_dashboard_logs))
        .route("/dashboard/sessions", get(handle_dashboard_sessions))
        .route("/dashboard/engine/{mod_id}", get(handle_engine_mod))
        .route("/dashboard/engine/core/modules/{mod_id}/toggle", post(handle_module_toggle))
        .route("/dashboard/engine/core/restart", post(handle_restart))
        .route("/dashboard/engine/tools/toggle", post(handle_tools_toggle))
        .route("/dashboard/engine/tools/admin", post(handle_tools_admin))
        .route("/dashboard/emotion", get(handle_emotion_status))
        .route("/dashboard/emotion/update", post(handle_emotion_update))
        .route("/dashboard/emotion/fluctuate", post(handle_emotion_fluctuate))
        .route("/dashboard/emotion/interval", get(handle_emotion_interval_get))
        .route("/dashboard/emotion/interval", post(handle_emotion_interval_set))
        .route("/dashboard/cron/targets", get(handle_channel_targets))
        .route("/dashboard/cron/tasks", get(handle_cron_tasks))
        .route("/dashboard/cron/create", post(handle_cron_create))
        .route("/dashboard/cron/{idx}/toggle", post(handle_cron_toggle))
        .route("/dashboard/cron/{idx}/delete", post(handle_cron_delete))
        .route("/dashboard/cron/{idx}/update", post(handle_cron_update))
        .route("/dashboard/cron/{idx}/run", post(handle_cron_run))
        .route("/dashboard/delegation/settings", get(handle_delegation_settings_get).post(handle_delegation_settings_save))
        .route("/dashboard/delegation/acp/add", post(handle_acp_agent_add))
        .route("/dashboard/delegation/acp/remove", post(handle_acp_agent_remove))
        .route("/dashboard/mcp/servers", get(handle_mcp_servers))
        .route("/dashboard/channels/manage", get(handle_channels_manage))
        .route("/dashboard/channels/save", post(handle_channels_save))
        .route("/dashboard/channels/sync-config", post(handle_channels_sync_config))
        .route("/dashboard/engine/memory/search", get(handle_memory_search))
        .route("/dashboard/engine/memory/delete", post(handle_memory_delete))
        .route("/dashboard/engine/memory/update", post(handle_memory_update))
        .route("/dashboard/engine/memory/paths", get(handle_memory_paths))
        .route("/dashboard/config/profile", get(handle_profile_get).post(handle_profile_set))
        .route("/dashboard/attention/update", post(handle_attention_update))
        .route("/dashboard/compress/exec", post(handle_compress_exec))
        .route("/dashboard/compress/settings", get(handle_compress_settings_get).post(handle_compress_settings_ser))
        .route("/dashboard/session/config", post(handle_session_config_save))
        .route("/dashboard/session/toggle_share", post(handle_session_toggle_share))
        .route("/dashboard/session/activate", post(handle_session_activate))
        .route("/dashboard/session/cooldown", post(handle_session_cooldown))
        .route("/dashboard/config/check", get(handle_config_check))
        .route("/dashboard/metabolism", get(handle_metabolism_get).post(handle_metabolism_save))
        .route("/dashboard/config/avatar/upload", post(handle_avatar_upload))
        .route("/avatars/{*filename}", get(handle_avatar_serve))
        .route("/dashboard/plan", get(handle_plan_list))
        .route("/dashboard/plan/active", get(handle_plan_active))
        .route("/dashboard/plan/deactivate", post(handle_plan_deactivate))
        .route("/dashboard/plan/settings", get(handle_plan_settings_get))
        .route("/dashboard/plan/settings", post(handle_plan_settings_set))
        .route("/dashboard/plan/create", post(handle_plan_create))
        .route("/dashboard/plan/{id}", get(handle_plan_get))
        .route("/dashboard/plan/{id}/update", post(handle_plan_update))
        .route("/dashboard/plan/{id}/delete", post(handle_plan_delete))
        .route("/dashboard/plan/{id}/activate", post(handle_plan_activate))
        // ─── 用户画像 CRUD ────────────────────
        .route("/dashboard/user/{uid}", get(handle_user_get))
        .route("/dashboard/user/create", post(handle_user_create))
        .route("/dashboard/user/{uid}/update", post(handle_user_update))
        .route("/dashboard/user/{uid}/delete", post(handle_user_delete))
        .route("/dashboard/user/{uid}/rename", post(handle_user_rename))
        .route("/dashboard/user/settings", get(handle_user_settings_get))
        .route("/dashboard/user/settings", post(handle_user_settings_set))
        .layer(CorsLayer::permissive())
        .layer(Extension(state.clone()));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("Failed to bind {addr}: {e}"))?;

    println!("\n  Tremolite HTTP daemon started. Listening on http://{addr}");
    println!("  GET  /health        —  health check (detailed)");
    println!("  GET  /metrics       —  server metrics");
    println!("  GET  /dashboard     —  web dashboard UI");
    println!("  GET  /dashboard/status  —  dashboard JSON API");
    println!("  POST /dashboard/profiles/load  —  switch agent profile");
    println!("  POST /chat          —  send message to agent");
    println!("  WS   /ws            —  WebSocket chat");
    println!("  GET  /avatars/*     —  avatar static files");
    println!("  Press Ctrl+C to gracefully shut down.");

    // 优雅关停
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let notify_for_signal = shutdown_notify.clone();

    // 在后台等待 SIGTERM / SIGINT
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let mut sigterm = signal(SignalKind::terminate())
                .expect("Failed to register SIGTERM handler");
            let mut sigint = signal(SignalKind::interrupt())
                .expect("Failed to register SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => {
                    println!("\n  Received SIGTERM. Shutting down gracefully...");
                }
                _ = sigint.recv() => {
                    println!("\n  Received SIGINT. Shutting down gracefully...");
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            println!("\n  Received Ctrl+C. Shutting down gracefully...");
        }

        notify_for_signal.notify_one();
    });

    // 用 axum::serve with graceful shutdown
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown_notify.notified().await;
            println!("  Flushing state and shutting down...");
            tokio::time::sleep(Duration::from_millis(500)).await;
        })
        .await
        .map_err(|e| format!("Server error: {e}"))?;

    println!("  Tremolite daemon stopped.");
    Ok(())
}

// ─── 从配置初始化通道 ──────────────────────────────

pub fn initialize_channels(
    channels_config: &HashMap<String, tremolite_config::ChannelConfig>,
    profile_name: &str,
) -> ChannelRegistry {
    let mut registry = ChannelRegistry::new();

    for (key, config) in channels_config {
        match config {
            tremolite_config::ChannelConfig::Http { listen, name, broadcast_target: _ } => {
                let channel_name = name.clone().unwrap_or_else(|| key.clone());
                let channel = tremolite_channels::HttpChannel::new(&channel_name, listen);

                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let reg = &mut registry;
                    let ch = Box::new(channel);
                    handle.block_on(async move {
                        if let Err(e) = reg.register(ch).await {
                            tracing::warn!("channel '{}': failed to register: {}", channel_name, e);
                        } else {
                            tracing::info!("channel '{}': HttpChannel registered on {}", channel_name, listen);
                        }
                    });
                } else {
                    tracing::warn!(
                        "channel '{}': no tokio runtime, skipping (daemon mode only)",
                        channel_name
                    );
                }
            }
            tremolite_config::ChannelConfig::NapCat { ws_url, name, broadcast_target } => {
                let channel_name = name.clone().unwrap_or_else(|| key.clone());
                let home = std::env::var("HOME").unwrap_or_default();
                let targets_path = std::path::Path::new(&home)
                    .join(".tremolite").join("profiles").join(profile_name)
                    .join("channels").join(format!("{}_targets.json", channel_name));
                let channel = tremolite_channels::NapCatChannel::new(&channel_name, ws_url)
                    .with_broadcast_target(broadcast_target.clone())
                    .with_targets_path(Some(targets_path));

                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let reg = &mut registry;
                    let ch = Box::new(channel);
                    handle.block_on(async move {
                        if let Err(e) = reg.register(ch).await {
                            tracing::warn!("channel '{}': failed to register: {}", channel_name, e);
                        } else {
                            tracing::info!(
                                "channel '{}': NapCatChannel registered with ws={}",
                                channel_name, ws_url
                            );
                        }
                    });
                } else {
                    tracing::warn!(
                        "channel '{}': no tokio runtime, skipping (daemon mode only)",
                        channel_name
                    );
                }
            }
            tremolite_config::ChannelConfig::QqBot { app_id, client_secret, token: _token, production, name, broadcast_target } => {
                let channel_name = name.clone().unwrap_or_else(|| key.clone());
                let home = std::env::var("HOME").unwrap_or_default();
                let targets_path = std::path::Path::new(&home)
                    .join(".tremolite").join("profiles").join(profile_name)
                    .join("channels").join(format!("{}_targets.json", channel_name));
                let channel = tremolite_channels::QqBotChannel::new(
                    &channel_name, app_id, client_secret, *production,
                ).with_broadcast_target(broadcast_target.clone())
                 .with_targets_path(Some(targets_path));

                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    let reg = &mut registry;
                    let ch = Box::new(channel);
                    handle.block_on(async move {
                        if let Err(e) = reg.register(ch).await {
                            tracing::warn!("channel '{}': failed to register: {}", channel_name, e);
                        } else {
                            tracing::info!(
                                "channel '{}': QqBotChannel registered (production={})",
                                channel_name, production
                            );
                        }
                    });
                } else {
                    tracing::warn!(
                        "channel '{}': no tokio runtime, skipping (daemon mode only)",
                        channel_name
                    );
                }
            }
        }
    }

    registry
}

// ─── Handler：增强版健康检查 ──────────────────────

async fn handle_health(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    let uptime = state.uptime_secs();

    // 内存信息
    let mem_info = get_memory_info();

    Json(serde_json::json!({
        "status": "ok",
        "service": "tremolite",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_secs": uptime,
        "uptime_human": format_uptime(uptime),
        "mode": "daemon",
        "metrics": {
            "total_requests": state.total_requests.load(Ordering::Relaxed),
            "active_connections": state.active_connections.load(Ordering::Relaxed),
        },
        "memory": mem_info,
    }))
}

// ─── Handler：指标 ─────────────────────────────

async fn handle_metrics(Extension(state): Extension<Arc<AppState>>) -> Json<Value> {
    let uptime = state.uptime_secs();

    Json(serde_json::json!({
        "uptime_secs": uptime,
        "total_requests": GLOBAL_METRICS.total_requests.load(Ordering::Relaxed),
        "total_errors": GLOBAL_METRICS.total_errors.load(Ordering::Relaxed),
        "total_tool_calls": GLOBAL_METRICS.total_tool_calls.load(Ordering::Relaxed),
        "total_llm_calls": GLOBAL_METRICS.total_llm_calls.load(Ordering::Relaxed),
        "memory": get_memory_info(),
    }))
}

// ─── Handler：聊天 API ───────────────────────────

#[derive(serde::Deserialize)]
struct ChatRequest {
    message: String,
    #[serde(default)]
    session_id: Option<String>,
}

async fn handle_chat(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<ChatRequest>,
) -> Json<Value> {
    GLOBAL_METRICS.total_requests.fetch_add(1, Ordering::Relaxed);

    let session_id = payload.session_id.unwrap_or_else(|| "http-default".to_string());
    let pending_id = format!("pending-http-{}", SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos());

    // 注册待返回结果
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    {
        let mut map = match state.pending_results.lock() {
            Ok(m) => m,
            Err(_) => {
                GLOBAL_METRICS.total_errors.fetch_add(1, Ordering::Relaxed);
                return Json(serde_json::json!({
                    "error": "Pending results lock contention",
                    "status": "error"
                }));
            }
        };
        map.insert(pending_id.clone(), result_tx);
    }

    // 投递到调度器
    let task = SessionTask {
        session_id,
        input: payload.message,
        channel: "http".into(),
        sender: pending_id,
    };
    if state.inbound_tx.send(task).is_err() {
        GLOBAL_METRICS.total_errors.fetch_add(1, Ordering::Relaxed);
        return Json(serde_json::json!({
            "error": "Scheduler unavailable",
            "status": "error"
        }));
    }

    // 轮询等回复（60 秒超时）
    let deadline = SystemTime::now() + Duration::from_secs(60);
    let result = loop {
        if SystemTime::now() >= deadline {
            break prompts::llm_timeout();
        }
        match result_rx.try_recv() {
            Ok(reply) => break reply,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
            Err(_) => break prompts::channel_closed(),
        }
    };

    Json(serde_json::json!({
        "response": result,
        "status": "ok",
    }))
}

// ─── Handler：WebSocket ───────────────────────────

async fn handle_ws(
    Extension(state): Extension<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_socket(socket, state))
}

async fn handle_ws_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    state.active_connections.fetch_add(1, Ordering::Relaxed);

    while let Some(msg) = receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                let session_id = "ws-default".to_string();
                let pending_id = format!("pending-ws-{}", SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos());

                // 注册待返回结果
                let (result_tx, result_rx) = std::sync::mpsc::channel();
                if let Ok(mut map) = state.pending_results.lock() {
                    map.insert(pending_id.clone(), result_tx);
                }

                // 投递到调度器
                let task = SessionTask {
                    session_id,
                    input: text.to_string(),
                    channel: "websocket".into(),
                    sender: pending_id,
                };
                let _ = state.inbound_tx.send(task);

                // 轮询等回复（30 秒超时）
                let start = SystemTime::now();
                let response = loop {
                    match result_rx.try_recv() {
                        Ok(reply) => break reply,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            if start.elapsed().unwrap_or_default().as_secs() > 30 {
                                break "[Timeout]".to_string();
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                        Err(_) => break "[Error]".to_string(),
                    }
                };
                let _ = sender.send(Message::Text(response.into())).await;
            }
            Ok(Message::Close(_)) => break,
            _ => {}
        }
    }

    state.active_connections.fetch_sub(1, Ordering::Relaxed);
}

// ─── Handler：Dashboard ─────────────────────────

async fn handle_dashboard() -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate"),
    );
    headers.insert(
        header::PRAGMA,
        HeaderValue::from_static("no-cache"),
    );
    headers.insert(
        header::EXPIRES,
        HeaderValue::from_static("0"),
    );
    (headers, Html(dashboard_html()))
}

const EMOTION_PATH: &str = "/home/spicysugar/.tremolite/data/emotion.json";

// ─── Plan 数据路径常量 ──────────────────────────
const PLAN_DIR: &str = ".tremolite/data/tremolite/plan";
const PLANS_FILE: &str = "plans.json";
const PLANS_CONFIG_FILE: &str = "plans_config.json";

fn plan_dir(home: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(home).join(PLAN_DIR)
}
fn plans_path(home: &str) -> std::path::PathBuf {
    plan_dir(home).join(PLANS_FILE)
}
fn plans_config_path(home: &str) -> std::path::PathBuf {
    plan_dir(home).join(PLANS_CONFIG_FILE)
}
fn read_plans(home: &str) -> Vec<serde_json::Value> {
    let path = plans_path(home);
    if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    }
}
fn write_plans(home: &str, plans: &Vec<serde_json::Value>) {
    let dir = plan_dir(home);
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(plans_path(home), serde_json::to_string_pretty(plans).unwrap()).ok();
}
fn read_plan_config(home: &str) -> serde_json::Value {
    let path = plans_config_path(home);
    if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        serde_json::json!({
            "auto_detect_enabled": false,
            "auto_detect_threshold": 0.6,
            "auto_switch_cooldown_secs": 300
        })
    }
}
fn write_plan_config(home: &str, config: &serde_json::Value) {
    let dir = plan_dir(home);
    std::fs::create_dir_all(&dir).ok();
    std::fs::write(plans_config_path(home), serde_json::to_string_pretty(config).unwrap()).ok();
}

async fn handle_dashboard_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<Value> {
    let uptime = state.uptime_secs();

    // 读情绪数据
    let emotion_data = std::fs::read_to_string(EMOTION_PATH).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_default();
    let plutchik = emotion_data.get("plutchik").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let energy = emotion_data.get("energy").and_then(|v| v.as_f64()).unwrap_or(50.0);

    // 找主导情绪
    let dominant = plutchik.iter()
        .max_by(|a, b| a.1.as_f64().unwrap_or(0.0).partial_cmp(&b.1.as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(k, _)| k.as_str())
        .unwrap_or("trust");

    // 读记忆统计（尝试读 l2_profile）
    let home = std::env::var("HOME").unwrap_or_default();
    let (memory_dir, _) = memory_dir_path(&home);
    let l2_path = memory_dir.join("l2_profile.json");
    let memory_total = std::fs::read_to_string(&l2_path).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v.as_object().map(|o| o.len()).unwrap_or(0))
        .unwrap_or(0);

    // 从注册表文件读取实际安装的模块
    let home = std::env::var("HOME").unwrap_or_default();
    let reg_path = std::path::PathBuf::from(&home)
        .join(".tremolite").join("data").join("tremolite").join("modules_registry.json");
    let registered_mods: Vec<serde_json::Value> = std::fs::read_to_string(&reg_path).ok()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .unwrap_or_default();
    let mut module_entries: Vec<serde_json::Value> = Vec::new();
    // 核心引擎固定显示
    module_entries.push(serde_json::json!({"name": "core", "label": "核心引擎", "version": tremolite_core::CORE_VERSION}));
    for m in &registered_mods {
        let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(id);
        let version = m.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        module_entries.push(serde_json::json!({"name": id, "label": name, "version": version}));
    }
    // 去重（避免 dashboard 等模块在两种模式下重复注册）
    let mut seen = std::collections::HashSet::new();
    module_entries.retain(|e| {
        let n = e.get("name").and_then(|v| v.as_str()).unwrap_or("");
        seen.insert(n.to_string())
    });
    // 固定排序：core 在最前，其余按 name 字母序
    module_entries.sort_by(|a, b| {
        let na = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let nb = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if na == "core" { std::cmp::Ordering::Less }
        else if nb == "core" { std::cmp::Ordering::Greater }
        else { na.cmp(nb) }
    });

    // 读 profiles 列表
    let home = std::env::var("HOME").unwrap_or_default();
    let profiles_base = std::path::Path::new(&home).join(".tremolite").join("profiles");
    // 从 config.toml 读当前激活的包名
    let active_profile = std::fs::read_to_string(
        std::path::Path::new(&home).join(".tremolite").join("config.toml")
    ).ok()
        .and_then(|s| {
            s.lines()
                .skip_while(|l| !l.trim().starts_with("[profile]"))
                .nth(1)
                .and_then(|l| l.trim().strip_prefix("name = "))
                .map(|v| v.trim_matches('"').to_string())
        })
        .unwrap_or_else(|| "main".into());
    let mut profiles_list: Vec<serde_json::Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&profiles_base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let is_active = name == active_profile;
                // 读 SOUL.md 前几行作为描述
                let soul_preview = std::fs::read_to_string(path.join("SOUL.md"))
                    .ok()
                    .map(|s| {
                        s.lines()
                            .filter(|l| !l.trim_start().starts_with('#'))
                            .filter(|l| !l.trim().is_empty())
                            .take(3)
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(120)
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                // 技能数
                let skills_count = std::fs::read_dir(path.join("skills"))
                    .map(|d| d.flatten().count())
                    .unwrap_or(0);
                profiles_list.push(serde_json::json!({
                    "name": name,
                    "active": is_active,
                    "soul_preview": soul_preview,
                    "skills_count": skills_count,
                }));
            }
        }
    }
    profiles_list.sort_by(|a, b| {
        let a_active = a.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        let b_active = b.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        b_active.cmp(&a_active)
    });

    // 获取 UserModule 的上次注入内容
    let user_injected = state.modules.as_ref()
        .and_then(|mods| mods.with_module("user", |m| {
            m.as_any()
                .and_then(|any| any.downcast_ref::<tremolite_core::UserModule>())
                .map(|um| um.last_injected.clone())
        }))
        .flatten()
        .unwrap_or_default();

    Json(serde_json::json!({
        "user_injected": user_injected,
        "status": "ok",
        "system": {
            "status": "ok",
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": uptime,
        },
        "uptime_human": format_uptime(uptime),
        "metrics": {
            "total_requests": GLOBAL_METRICS.total_requests.load(Ordering::Relaxed),
            "active_connections": state.active_connections.load(Ordering::Relaxed),
        },
        "emotion": {
            "display": dominant,
            "composite": dominant,
            "dominant": dominant,
            "dimensions": plutchik,
            "energy": energy,
        },
        "memory": {
            "total_entries": memory_total,
            "l1": 0,
            "l2": memory_total,
            "l3": 0,
            "ram": 0,
        },
        "skills": {
            "count": 0,
            "total_practices": 0,
        },
        "modules": {
            "registered": module_entries,
        },
        "profiles": profiles_list,
        "profile": {
            "name": active_profile,
        },
        "llm": {
            "providers": ["deepseek"],
            "default": "deepseek-v4-flash",
        },
        "conversation": [],
    }))
}

// ─── Handler: 引擎模块详情 ─────────────────────
//
// 每个模块页展示专属数据，而不是统一的系统概览 fallback
use axum::extract::Path;


#[derive(serde::Deserialize)]
struct ToolsToggleRequest {
    name: String,
    enabled: bool,
}

async fn handle_tools_toggle(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<ToolsToggleRequest>,
) -> Json<serde_json::Value> {
    let config_path = tools_config_path(&state);
    let content = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => return Json(serde_json::json!({"status": "error", "error": "找不到 tools.toml"})),
    };
    let found_existing = content.contains("[[tool_settings]]")
        && content.lines().zip(content.lines().skip(1)).any(|(prev, curr)| {
            prev.trim() == "[[tool_settings]]"
            && curr.trim() == format!("name = \"{}\"", payload.name)
        });
    let settings_entry = format!("[[tool_settings]]\nname = \"{}\"\nenabled = {}", payload.name, if payload.enabled { "true" } else { "false" });
    let new_content = if found_existing {
        let lines: Vec<&str> = content.lines().collect();
        let mut result: Vec<String> = Vec::new();
        let mut i = 0;
        let name_match = format!("name = \"{}\"", payload.name);
        while i < lines.len() {
            let line = lines[i];
            if line.trim() == name_match.as_str() {
                if result.last().map(|s| s.trim()) == Some("[[tool_settings]]") { result.pop(); }
                i += 1;
                if i < lines.len() && lines[i].trim().starts_with("enabled") { i += 1; }
                result.push("[[tool_settings]]".to_string());
                result.push(format!("name = \"{}\"", payload.name));
                result.push(format!("enabled = {}", if payload.enabled { "true" } else { "false" }));
            } else { result.push(line.to_string()); }
            i += 1;
        }
        result.join("\n")
    } else {
        let trimmed = content.trim_end();
        format!("{}{}\n\n{}\n", trimmed, if trimmed.ends_with('\n') { "" } else { "\n" }, settings_entry)
    };
    match std::fs::write(&config_path, &new_content) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "name": payload.name, "enabled": payload.enabled})),
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})),
    }
}

#[derive(serde::Deserialize)]
struct ToolsAdminRequest {
    action: String,
    #[serde(default)]
    category_id: String,
    #[serde(default)]
    category_label: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    tool_new_name: String,
    #[serde(default)]
    tool_description: String,
    #[serde(default)]
    tool_parameters: String,
}

fn tools_config_path(state: &AppState) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".tremolite").join("profiles").join(&state.profile_name).join("modules").join("tools.toml")
}

async fn handle_tools_admin(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<ToolsAdminRequest>,
) -> Json<serde_json::Value> {
    let config_path = tools_config_path(&state);
    let config_str = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(_) => return Json(serde_json::json!({"status": "error", "error": "找不到 tools.toml"})),
    };
    let lines: Vec<&str> = config_str.lines().collect();
    match payload.action.as_str() {
        "create_category" => {
            if payload.category_id.is_empty() { return Json(serde_json::json!({"status": "error", "error": "缺少 category_id"})); }
            let label = if payload.category_label.is_empty() { &payload.category_id } else { &payload.category_label };
            match std::fs::write(&config_path, format!("{}\n[[category]]\nid = \"{}\"\nlabel = \"{}\"\ntools = []\n", config_str.trim_end(), payload.category_id, label)) {
                Ok(_) => Json(serde_json::json!({"status": "ok"})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})),
            }
        }
        "update_category" => {
            if payload.category_id.is_empty() { return Json(serde_json::json!({"status": "error", "error": "缺少 category_id"})); }
            let mut r: Vec<String> = Vec::new();
            let id_line = format!("id = \"{}\"", payload.category_id);
            let mut found = false; let mut in_block = false;
            for line in lines {
                let t = line.trim();
                if t == "[[category]]" { in_block = true; r.push(line.to_string()); }
                else if in_block && t == id_line { found = true; r.push(line.to_string()); in_block = false; }
                else if in_block && t.starts_with("id = \"") { in_block = false; r.push(line.to_string()); }
                else if found && t.starts_with("label = \"") {
                    r.push(format!("label = \"{}\"", if payload.category_label.is_empty() { &payload.category_id } else { &payload.category_label }));
                    found = false;
                } else { r.push(line.to_string()); }
            }
            match std::fs::write(&config_path, r.join("\n")) {
                Ok(_) => Json(serde_json::json!({"status": "ok"})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})),
            }
        }
        "delete_category" => {
            if payload.category_id.is_empty() { return Json(serde_json::json!({"status": "error", "error": "缺少 category_id"})); }
            let mut r: Vec<String> = Vec::new();
            let id_line = format!("id = \"{}\"", payload.category_id);
            let mut skip = false; let mut i = 0;
            while i < lines.len() {
                if lines[i].trim() == "[[category]]" && i+1 < lines.len() && lines[i+1].trim() == id_line {
                    skip = true; i += 2;
                    while i < lines.len() { let t = lines[i].trim(); if t.starts_with("[[") || t.is_empty() { break; } i += 1; }
                    continue;
                }
                if !skip { r.push(lines[i].to_string()); }
                skip = false; i += 1;
            }
            match std::fs::write(&config_path, r.join("\n")) {
                Ok(_) => Json(serde_json::json!({"status": "ok"})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})),
            }
        }
        "add_tool" => {
            if payload.tool.is_empty() { return Json(serde_json::json!({"status": "error", "error": "缺少 tool"})); }
            let id_line = format!("id = \"{}\"", payload.category_id);
            let mut r: Vec<String> = Vec::new(); let mut in_target = false;
            for line in lines {
                let t = line.trim();
                if t == "[[category]]" { in_target = false; r.push(line.to_string()); }
                else if t == id_line { in_target = true; r.push(line.to_string()); }
                else if in_target && (t.starts_with("tools = [") || t.starts_with("tools=[")) {
                    if let Some(start) = line.find("= [") {
                        let prefix = &line[..start+3];
                        let rest = &line[start+3..];
                        if let Some(end) = rest.rfind(']') {
                            let inner = rest[..end].trim();
                            let add = if inner.is_empty() || inner == "]" { format!("\"{}\"", payload.tool) } else { format!("{}, \"{}\"", inner, payload.tool) };
                            r.push(format!("{} {}]", prefix, add));
                        } else { r.push(line.to_string()); }
                    } else { r.push(line.to_string()); }
                    in_target = false;
                } else { r.push(line.to_string()); }
            }
            match std::fs::write(&config_path, r.join("\n")) {
                Ok(_) => Json(serde_json::json!({"status": "ok", "tool": payload.tool})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})),
            }
        }
        "delete_tool" => {
            if payload.tool.is_empty() { return Json(serde_json::json!({"status": "error", "error": "缺少 tool"})); }
            let id_line = format!("id = \"{}\"", payload.category_id);
            let mut r: Vec<String> = Vec::new(); let mut in_target = false;
            for line in lines {
                let t = line.trim();
                if t == "[[category]]" { in_target = false; r.push(line.to_string()); }
                else if t == id_line { in_target = true; r.push(line.to_string()); }
                else if in_target && (t.starts_with("tools = [") || t.starts_with("tools=[")) {
                    if let Some(start) = line.find("= [") {
                        let prefix = &line[..start+3];
                        let rest = &line[start+3..];
                        if let Some(end) = rest.rfind(']') {
                            let items: Vec<&str> = rest[..end].trim().split(',').map(|s| s.trim().trim_matches('"')).filter(|s| !s.is_empty() && *s != payload.tool).collect();
                            if items.is_empty() { r.push(format!("{}]", prefix)); }
                            else { r.push(format!("{} {}]", prefix, items.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", "))); }
                        } else { r.push(line.to_string()); }
                    } else { r.push(line.to_string()); }
                    in_target = false;
                } else { r.push(line.to_string()); }
            }
            match std::fs::write(&config_path, r.join("\n")) {
                Ok(_) => Json(serde_json::json!({"status": "ok"})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})),
            }
        }
        "update_tool" => {
            if payload.tool.is_empty() { return Json(serde_json::json!({"status": "error", "error": "缺少 tool"})); }
            let old_name = &payload.tool;
            let new_name = if payload.tool_new_name.is_empty() { old_name } else { &payload.tool_new_name };
            let id_line = format!("id = \"{}\"", payload.category_id);
            let mut r: Vec<String> = Vec::new(); let mut in_target = false;
            let mut renamed = false;
            for line in &lines {
                let t = line.trim();
                if t == "[[category]]" { in_target = false; r.push(line.to_string()); }
                else if t == id_line { in_target = true; r.push(line.to_string()); }
                else if in_target && (t.starts_with("tools = [") || t.starts_with("tools=[")) && old_name != new_name {
                    if let Some(start) = line.find("= [") {
                        let prefix = &line[..start+3];
                        let rest = &line[start+3..];
                        if let Some(end) = rest.rfind(']') {
                            let items: Vec<String> = rest[..end].trim().split(',').map(|s| s.trim().trim_matches('"').to_string()).map(|s| if s == *old_name { new_name.to_string() } else { s }).filter(|s| !s.is_empty()).collect();
                            r.push(format!("{} {}]", prefix, items.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ")));
                            renamed = true;
                        } else { r.push(line.to_string()); }
                    } else { r.push(line.to_string()); }
                    in_target = false;
                } else { r.push(line.to_string()); }
            }
            if renamed {
                let mut s: Vec<String> = Vec::new();
                let old_match = format!("name = \"{}\"", old_name);
                for line in &r {
                    if line.trim() == old_match.as_str() { s.push(format!("name = \"{}\"", new_name)); }
                    else { s.push(line.to_string()); }
                }
                r = s;
            }
            let meta_name = if renamed { new_name } else { old_name };
            let has_desc = !payload.tool_description.is_empty();
            let has_params = !payload.tool_parameters.is_empty();
            if has_desc || has_params {
                let mm = "[[tool_meta]]"; let mn = format!("name = \"{}\"", meta_name);
                let mut found = false; let mut mr: Vec<String> = Vec::new(); let mut i = 0;
                while i < r.len() {
                    if r[i].trim() == mm && i+1 < r.len() && r[i+1].trim() == mn {
                        found = true; mr.push("[[tool_meta]]".to_string()); mr.push(format!("name = \"{}\"", meta_name));
                        if has_desc { mr.push(format!("description = \"{}\"", payload.tool_description.replace('"', "\\\""))); }
                        if has_params { mr.push(format!("parameters = \"{}\"", payload.tool_parameters.replace('"', "\\\""))); }
                        i += 2; while i < r.len() { let tt = r[i].trim(); if tt.starts_with("[[") || tt.is_empty() { break; } i += 1; }
                        continue;
                    }
                    mr.push(r[i].clone()); i += 1;
                }
                if !found {
                    mr.push(String::new()); mr.push("[[tool_meta]]".to_string()); mr.push(format!("name = \"{}\"", meta_name));
                    if has_desc { mr.push(format!("description = \"{}\"", payload.tool_description.replace('"', "\\\""))); }
                    if has_params { mr.push(format!("parameters = \"{}\"", payload.tool_parameters.replace('"', "\\\""))); }
                    mr.push(String::new());
                }
                match std::fs::write(&config_path, mr.join("\n")) { Ok(_) => Json(serde_json::json!({"status": "ok", "renamed": renamed})), Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})), }
            } else {
                match std::fs::write(&config_path, r.join("\n")) { Ok(_) => Json(serde_json::json!({"status": "ok", "renamed": renamed})), Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入失败: {e}")})), }
            }
        }
        _ => Json(serde_json::json!({"status": "error", "error": format!("未知 action: {}", payload.action)})),
    }
}

async fn handle_engine_mod(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Path(mod_id): Path<String>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tremolite_dir = std::path::Path::new(&home).join(".tremolite");

    // 读情绪数据（多个模块共用）
    let emotion_data = std::fs::read_to_string(tremolite_dir.join("data").join("emotion.json")).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_default();
    let plutchik = emotion_data.get("plutchik").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let energy = emotion_data.get("energy").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let dominant = plutchik.iter()
        .max_by(|a, b| a.1.as_f64().unwrap_or(0.0).partial_cmp(&b.1.as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(k, _)| k.as_str())
        .unwrap_or("trust");

    // 读记忆统计
    let (memory_dir, _) = memory_dir_path(&home);
    let l2_path = memory_dir.join("l2_profile.json");
    let _mem_total = std::fs::read_to_string(&l2_path).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| v.as_object().map(|o| o.len()).unwrap_or(0))
        .unwrap_or(0);

    let data = match mod_id.as_str() {
        "core" => {
            // 读 LLM 配置
            let llm_model = std::fs::read_to_string(tremolite_dir.join("config.toml")).ok()
                .and_then(|s| {
                    for line in s.lines() {
                        let t = line.trim();
                        if let Some(v) = t.strip_prefix("default = \"") {
                            if let Some(end) = v.find('"') {
                                return Some(v[..end].to_string());
                            }
                        } else if t.starts_with("default = '") {
                            if let Some(end) = t[11..].find('\'') {
                                return Some(t[11..11+end].to_string());
                            }
                        }
                    }
                    None
                })
                .unwrap_or_else(|| "deepseek-v4-flash".into());
            // 读模块状态
            let state_path = tremolite_dir.join("data").join("tremolite").join("modules_state.json");
            let mod_states = std::fs::read_to_string(&state_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::json!({}));
            let mut mod_list = Vec::new();
            // 从注册表文件读取实际安装的模块
            let reg_path = tremolite_dir.join("data").join("tremolite").join("modules_registry.json");
            let registered_mods: Vec<serde_json::Value> = std::fs::read_to_string(&reg_path).ok()
                .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
                .unwrap_or_default();
            // 核心引擎固定显示
            mod_list.push(serde_json::json!({
                "id": "core",
                "name": "核心引擎",
                "enabled": mod_states.get("core").and_then(|v| v.as_bool()).unwrap_or(true),
            }));
            for m in &registered_mods {
                let mid = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
                let name = m.get("name").and_then(|v| v.as_str()).unwrap_or(mid);
                let enabled = mod_states.get(mid).and_then(|v| v.as_bool()).unwrap_or(true);
                mod_list.push(serde_json::json!({
                    "id": mid,
                    "name": name,
                    "enabled": enabled,
                }));
            }
            // 去重
            let mut seen = std::collections::HashSet::new();
            mod_list.retain(|e| {
                let n = e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                seen.insert(n)
            });
            serde_json::json!({
                "LLM模型": llm_model,
                "注册模块数": mod_list.len(),
                "模块启用数": mod_list.iter().filter(|m| m.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)).count(),
                "版本": tremolite_core::CORE_VERSION,
                "状态": "运行中",
                "运行时间": format_uptime(state.uptime_secs()),
                "模块列表": mod_list,
            })
        },
        "emotion" => {
            let mut dims = serde_json::Map::new();
            for (k, v) in &plutchik {
                dims.insert(k.clone(), v.clone());
            }
            serde_json::json!({
                "情绪": dominant,
                "能量": format!("{:.1}", energy),
                "情绪维度": dims,
                "版本": EMOTION_VERSION,
            })
        }
        "cron" => {
            let cron_path = tremolite_dir.join("profiles").join(&state.profile_name).join("cron_tasks.json");
            let tasks: Vec<serde_json::Value> = std::fs::read_to_string(&cron_path).ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let active = tasks.iter().filter(|t| t.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false)).count();
            serde_json::json!({
                "任务总数": tasks.len(),
                "活跃任务": active,
                "任务列表": tasks,
                "存储路径": cron_path.to_string_lossy(),
                "版本": CRON_VERSION,
            })
        }
        "memory" => {
            let trunc = |s: &str| -> String { s.chars().take(100).collect() };

            // L1: l1_working.json (可以是对象 session_id->array 或数组)
            let l1_entries: Vec<serde_json::Value> = std::fs::read_to_string(memory_dir.join("l1_working.json")).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|v| {
                    if let Some(arr) = v.as_array() {
                        arr.clone()
                    } else if let Some(obj) = v.as_object() {
                        obj.values()
                            .filter_map(|val| val.as_array().map(|a| a.clone()))
                            .flatten()
                            .collect()
                    } else {
                        vec![]
                    }
                })
                .unwrap_or_default();
            let l1: Vec<serde_json::Value> = l1_entries.iter().map(|e| {
                serde_json::json!({
                    "content": trunc(e.get("content").and_then(|v| v.as_str()).unwrap_or("")),
                    "tags": e.get("tags").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                    "importance": e.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    "source": e.get("source").and_then(|v| v.as_str()).unwrap_or(""),
                    "created_at": e.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                })
            }).collect::<Vec<_>>();
            // 按时间降序排列（最新的在最前）
            let mut l1 = l1;
            l1.sort_by(|a, b| {
                let ca = a.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let cb = b.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
                cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
            });

            // L2: l2_profile.json (HashMap<string, entry>)
            let l2: Vec<serde_json::Value> = std::fs::read_to_string(memory_dir.join("l2_profile.json")).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|v| {
                    v.as_object().map(|obj| {
                        obj.iter().map(|(key, val)| {
                            serde_json::json!({
                                "key": key,
                                "content": trunc(val.get("content").and_then(|v| v.as_str()).unwrap_or("")),
                                "tags": val.get("tags").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                                "importance": val.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.0),
                                "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            })
                        }).collect::<Vec<_>>()
                    }).unwrap_or_default()
                })
                .unwrap_or_default();

            // L3: l3_index.json (HashMap<u64, entry>)
            let l3: Vec<serde_json::Value> = std::fs::read_to_string(memory_dir.join("l3_index.json")).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|v| {
                    v.as_object().map(|obj| {
                        obj.iter().map(|(key, val)| {
                            serde_json::json!({
                                "id": key.parse::<u64>().unwrap_or(0),
                                "summary": trunc(val.get("summary").and_then(|v| v.as_str()).unwrap_or("")),
                                "has_embedding": val.get("embedding").is_some(),
                                "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            })
                        }).collect::<Vec<_>>()
                    }).unwrap_or_default()
                })
                .unwrap_or_default();

            // RAM: ram_fts.json (HashMap<u64, entry>)
            let ram: Vec<serde_json::Value> = std::fs::read_to_string(memory_dir.join("ram_fts.json")).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|v| {
                    v.as_object().map(|obj| {
                        obj.iter().map(|(key, val)| {
                            serde_json::json!({
                                "id": key.parse::<u64>().unwrap_or(0),
                                "content_preview": trunc(val.get("content_preview").and_then(|v| v.as_str()).unwrap_or("")),
                                "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            })
                        }).collect::<Vec<_>>()
                    }).unwrap_or_default()
                })
                .unwrap_or_default();

            // Disk: disk_index/index.json
            let disk: Vec<serde_json::Value> = std::fs::read_to_string(memory_dir.join("disk_index").join("index.json")).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|v| {
                    v.as_object().map(|obj| {
                        obj.iter().map(|(key, val)| {
                            serde_json::json!({
                                "id": key.parse::<u64>().unwrap_or(0),
                                "keyword": trunc(val.get("keyword").and_then(|v| v.as_str()).unwrap_or("")),
                                "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                            })
                        }).collect::<Vec<_>>()
                    }).unwrap_or_default()
                })
                .unwrap_or_default();

            let l1_count = l1.len();
            let l2_count = l2.len();
            let l3_count = l3.len();
            let ram_count = ram.len();
            let disk_count = disk.len();
            let total = l1_count + l2_count + l3_count + ram_count + disk_count;

            serde_json::json!({
                    "summary": {
                        "total_entries": total,
                        "l1": l1_count,
                        "l2": l2_count,
                        "l3": l3_count,
                        "ram": ram_count,
                        "disk": disk_count,
                        "storage_path": memory_dir.to_string_lossy().to_string(),
                    },
                    "layers": {
                        "l1": l1,
                        "l2": l2,
                        "l3": l3,
                        "ram": ram,
                        "disk": disk,
                    },
                    "metabolism": {
                        "demote_threshold": 0.3,
                        "promote_threshold": 0.7,
                        "weights": {
                            "l1": 1.0,
                            "l2": 2.0,
                            "l3": 3.0,
                            "ram": 1.5,
                            "disk": 0.5,
                        },
                    },
                    "version": MEMORY_VERSION,
                })
        }
        "tools" => {
            let profile_dir = tremolite_dir.join("profiles").join(&state.profile_name);
            let config_path = profile_dir.join("modules").join("tools.toml");
            let config_str = std::fs::read_to_string(&config_path).unwrap_or_default();
            let root: toml::Value = config_str.parse().unwrap_or(toml::Value::Table(toml::value::Table::new()));
            let tool_settings: std::collections::HashMap<String, bool> = root.get("tool_settings")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|item| {
                    let name = item.get("name").and_then(|v| v.as_str())?;
                    let enabled = item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
                    Some((name.to_string(), enabled))
                }).collect())
                .unwrap_or_default();
            let mut meta_registry = ToolRegistry::new();
            register_all(&mut meta_registry);
            let categories: Vec<serde_json::Value> = root.get("category")
                .and_then(|v| v.as_array().cloned().or_else(|| v.as_table().map(|t| vec![toml::Value::Table(t.clone())])))
                .unwrap_or_default()
                .iter().map(|item| {
                    let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let label = item.get("label").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
                    let tool_names: Vec<String> = item.get("tools")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    serde_json::json!({
                        "category": id, "label": label,
                        "tools": tool_names.iter().map(|t| {
                            let is_enabled = tool_settings.get(t).copied().unwrap_or(true);
                            let meta = meta_registry.get(t);
                            serde_json::json!({
                                "name": t, "enabled": is_enabled,
                                "description": meta.map(|m| m.description()).unwrap_or(""),
                                "parameters": meta.map(|m| m.parameters()).unwrap_or(serde_json::json!({})),
                            })
                        }).collect::<Vec<_>>(),
                        "tool_count": tool_names.len(),
                    })
                }).collect();
            let total: u64 = categories.iter().map(|c| c.get("tools").and_then(|v| v.as_array()).map(|a| a.len() as u64).unwrap_or(0)).sum();
            serde_json::json!({
                "模块ID": "tools", "状态": "内建模块", "版本": TOOLS_VERSION,
                "配置来源": config_path.to_string_lossy().to_string(),
                "工具分类": categories, "工具总数": total,
            })
        }
        "attention" => {
            let profile_dir = tremolite_dir.join("profiles").join(&state.profile_name);
            let app_config_path = profile_dir.join("config.toml");
            let app_config = std::fs::read_to_string(&app_config_path).ok().unwrap_or_default();

            let embedding_api_base = app_config.lines()
                .find(|l| l.trim().starts_with("api_base") || l.trim().starts_with("embedding_api_base"))
                .and_then(|l| l.split('=').nth(1)).map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| "未配置".into());
            let embedding_key = app_config.lines()
                .find(|l| l.trim().starts_with("api_key") || l.trim().starts_with("embedding_api_key") || l.trim().starts_with("api_key"))
                .and_then(|l| l.split('=').nth(1)).map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_default();
            let has_embedding_key = !embedding_key.is_empty();
            let embedding_key_mask = if embedding_key.len() > 10 {
                let prefix = &embedding_key[..4];
                let suffix = &embedding_key[embedding_key.len()-4..];
                format!("{}***{}", prefix, suffix)
            } else if !embedding_key.is_empty() {
                "***已配置***".to_string()
            } else {
                String::new()
            };
            let embedding_model = app_config.lines()
                .find(|l| l.trim().starts_with("model") || l.trim().starts_with("embedding_model"))
                .and_then(|l| l.split('=').nth(1)).map(|s| s.trim().trim_matches('"').to_string())
                .unwrap_or_else(|| "BAAI/bge-m3".into());

            let attn_weight_path = profile_dir.join("attention.json");
            let mut attention_weights = std::fs::read_to_string(&attn_weight_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("weights").cloned())
                .unwrap_or(serde_json::json!({
                    "wide": 1.0,
                    "focus": 2.0,
                    "micro": 3.0,
                }));

            let mut auto_tune = std::fs::read_to_string(&attn_weight_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("auto_tune").cloned())
                .unwrap_or(serde_json::json!({
                    "enabled": false,
                    "step_interval": 50,
                    "current_step": 0,
                    "last_adjust": null,
                    "last_top_score": null,
                }));

            if let Some(true) = auto_tune.get("enabled").and_then(|v| v.as_bool()) {
                let interval = auto_tune.get("step_interval").and_then(|v| v.as_u64()).unwrap_or(50);
                let step = auto_tune.get("current_step").and_then(|v| v.as_u64()).unwrap_or(0);
                let stats_path = profile_dir.join("attention_stats.json");
                let real_top_score = std::fs::read_to_string(&stats_path).ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    .and_then(|v| v.get("last_scan").and_then(|ls| ls.get("top_score")).and_then(|ts| ts.as_f64()));
                if step >= interval {
                    let mut weights_map = match &attention_weights {
                        serde_json::Value::Object(m) => m.clone(),
                        _ => serde_json::Map::new(),
                    };
                    let top_score = real_top_score.unwrap_or(0.5);
                    for (_, v) in weights_map.iter_mut() {
                        if let Some(w) = v.as_f64() {
                            let factor = if top_score < 0.3 {
                                1.1
                            } else if top_score > 0.7 {
                                0.95
                            } else {
                                1.0
                            };
                            let new_w = (w * factor).clamp(0.1, 5.0);
                            *v = serde_json::Value::Number(serde_json::Number::from_f64(
                                (new_w * 10.0).round() / 10.0
                            ).unwrap_or(serde_json::Number::from_f64(1.0).unwrap()));
                        }
                    }
                    let now_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let adjusted_auto_tune = serde_json::json!({
                        "enabled": true,
                        "step_interval": interval,
                        "current_step": 0,
                        "last_adjust": now_ts,
                        "last_top_score": (top_score * 100.0).round() / 100.0,
                    });
                    let updated_attn = serde_json::json!({
                        "weights": weights_map,
                        "auto_tune": adjusted_auto_tune,
                    });
                    let _ = std::fs::write(&attn_weight_path, serde_json::to_string_pretty(&updated_attn).unwrap());
                    attention_weights = updated_attn.get("weights").cloned().unwrap_or(attention_weights);
                    auto_tune = updated_attn.get("auto_tune").cloned().unwrap_or(auto_tune);
                } else {
                    let new_step = step + 1;
                    let mut current_data = std::fs::read_to_string(&attn_weight_path).ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .unwrap_or(serde_json::json!({}));
                    if let Some(obj) = current_data.as_object_mut() {
                        if let Some(at) = obj.get_mut("auto_tune").and_then(|v| v.as_object_mut()) {
                            at.insert("current_step".into(), serde_json::json!(new_step));
                        } else {
                            obj.insert("auto_tune".into(), serde_json::json!({
                                "enabled": true,
                                "step_interval": interval,
                                "current_step": new_step,
                            }));
                        }
                    }
                    let _ = std::fs::write(&attn_weight_path, serde_json::to_string_pretty(&current_data).unwrap());
                    auto_tune = current_data.get("auto_tune").cloned().unwrap_or(auto_tune);
                }
            }

            // 从 modules/attention.toml 读取通道配置
            let attention_config_path = profile_dir.join("modules").join("attention.toml");
            let channels: Vec<serde_json::Value> = std::fs::read_to_string(&attention_config_path).ok()
                .and_then(|s| {
                    let v: toml::Value = s.parse().ok()?;
                    v.get("attention")?.get("channels")?.as_array().map(|arr| {
                        arr.iter().map(|ch| {
                            let name = ch.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                            let label = ch.get("label").and_then(|n| n.as_str()).unwrap_or(name);
                            let window = ch.get("window").and_then(|n| n.as_integer()).unwrap_or(200) as u64;
                            let stride = ch.get("stride").and_then(|n| n.as_integer()).unwrap_or(50) as u64;
                            let max_blocks = ch.get("max_blocks").and_then(|n| n.as_integer()).unwrap_or(5) as u64;
                            let threshold = ch.get("threshold").and_then(|n| n.as_float()).unwrap_or(0.5);
                            serde_json::json!({
                                "name": name,
                                "label": label,
                                "window": window,
                                "stride": stride,
                                "max_blocks": max_blocks,
                                "threshold": (threshold * 100.0).round() / 100.0,
                                "description": format!("窗口 {} · 步长 {} · 阈值 {:.0}%", window, stride, threshold * 100.0),
                            })
                        }).collect::<Vec<_>>()
                    })
                })
                .unwrap_or_else(|| vec![
                    serde_json::json!({"name": "wide", "label": "全局视野", "window": 1000, "stride": 500, "max_blocks": 10, "threshold": 0.3, "description": "窗口 1000 · 步长 500 · 阈值 30%"}),
                    serde_json::json!({"name": "focus", "label": "焦点缩放", "window": 200, "stride": 50, "max_blocks": 8, "threshold": 0.5, "description": "窗口 200 · 步长 50 · 阈值 50%"}),
                    serde_json::json!({"name": "micro", "label": "微观精炼", "window": 50, "stride": 10, "max_blocks": 5, "threshold": 0.6, "description": "窗口 50 · 步长 10 · 阈值 60%"}),
                ]);

            // 读取 attention stats（只读，不写模拟数据）
            let stats_path = profile_dir.join("attention_stats.json");
            let stats_default = serde_json::json!({
                "history_count": 0,
                "total_tokens_scanned": 0,
                "last_scan": null,
            });
            if !stats_path.exists() {
                let _ = std::fs::write(&stats_path, serde_json::to_string_pretty(&stats_default).unwrap());
            }
            let stats = std::fs::read_to_string(&stats_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(stats_default.clone());
            let history_count = stats.get("history_count").and_then(|v| v.as_u64()).unwrap_or(0);
            let total_scanned = stats.get("total_tokens_scanned").and_then(|v| v.as_u64()).unwrap_or(0);
            let last_scan = stats.get("last_scan").cloned().unwrap_or(serde_json::Value::Null);
            let chain_depth = last_scan.get("chain_depth").and_then(|v| v.as_u64()).unwrap_or(0);
            let chat_type = last_scan.get("chat_type").and_then(|v| v.as_str()).unwrap_or("scattered");

            serde_json::json!({
                "模块ID": "attention",
                "版本": ATTENTION_VERSION,
                "状态": "运行中",
                "chain_depth": chain_depth,
                "chat_type": chat_type,
                "embedding": {
                    "api_base": embedding_api_base,
                    "api_key_mask": embedding_key_mask,
                    "model": embedding_model,
                    "has_embedding": has_embedding_key,
                    "configured": has_embedding_key,
                },
                "weights": attention_weights,
                "auto_tune": auto_tune,
                "channels": channels,
                "history_count": history_count,
                "last_scan": last_scan,
                "total_tokens_scanned": total_scanned,
                "injected_prompt": std::fs::read_to_string(profile_dir.join("attention_inject_log.txt")).ok().unwrap_or_default(),
            })
        }
        "skill" => {
            let skill_count = std::fs::read_to_string(tremolite_dir.join("data").join("learn").join("skills.json")).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .map(|v| if v.is_array() { v.as_array().map(|a| a.len()).unwrap_or(0) } else { v.as_object().map(|o| o.len()).unwrap_or(0) })
                .unwrap_or(0);
            serde_json::json!({
                "已注册技能": skill_count,
                "能力域": 0,
                "自动发现": "已启用",
                "版本": LEARN_VERSION,
            })
        }
        "reflection" => {
            let profile_dir = tremolite_dir.join("profiles").join(&state.profile_name);
            let config_path = profile_dir.join("modules").join("reflection.toml");

            let mut trigger_interval = 20i64;
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(toml_val) = content.parse::<toml::Value>() {
                    if let Some(cfg) = toml_val.get("reflection") {
                        trigger_interval = cfg.get("trigger_interval")
                            .and_then(|v| v.as_integer())
                            .unwrap_or(20);
                    }
                }
            }

            let l2_path = tremolite_dir.join("data").join("l2_profile.json");
            let l2_json = std::fs::read_to_string(&l2_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .unwrap_or(serde_json::json!({}));

            let dialectic_keys: Vec<String> = l2_json.as_object()
                .map(|obj| {
                    let mut keys: Vec<String> = obj.keys()
                        .filter(|k| k.contains("dialectic:"))
                        .cloned()
                        .collect();
                    keys.sort();
                    keys
                })
                .unwrap_or_default();

            let count = dialectic_keys.len();

            let recent_dialectics: Vec<serde_json::Value> = dialectic_keys.iter()
                .rev()
                .take(3)
                .filter_map(|k| {
                    l2_json.get(k).map(|entry| {
                        let content = entry.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let tags = entry.get("tags").and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect::<Vec<_>>())
                            .unwrap_or_default();
                        let summary = if tags.is_empty() {
                            content.chars().take(120).collect::<String>()
                        } else {
                            format!("[{}] {}", tags.join(", "), content.chars().take(100).collect::<String>())
                        };
                        serde_json::json!({
                            "time": k.strip_prefix("dialectic:").unwrap_or(k),
                            "summary": summary,
                        })
                    })
                })
                .collect();

            let profile_path = tremolite_dir.join("data").join("memory").join("profile.json");
            let profile_summary = std::fs::read_to_string(&profile_path).ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("summary").and_then(|s| s.as_str().map(|s| s.to_string())))
                .unwrap_or_else(|| "简要画像".to_string());

            serde_json::json!({
                "trigger_interval": trigger_interval,
                "dialectic_count": count,
                "recent_dialectics": recent_dialectics,
                "profile_summary": profile_summary,
                "配置来源": config_path.to_string_lossy().to_string(),
                "版本": REFLECTION_VERSION,
            })
        },
        "compress" => {
            let profile_dir = tremolite_dir.join("profiles").join(&state.profile_name);
            let config_path = profile_dir.join("modules").join("compress.toml");

            let mut blocks = 5usize;
            let mut ratios = vec!["Delete".to_string(), "20%".to_string(), "40%".to_string(), "60%".to_string(), "Full".to_string()];
            let mut threshold: u32 = 0;
            let mut auto_compress = true;

            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(toml_val) = content.parse::<toml::Value>() {
                    if let Some(cfg) = toml_val.get("compress") {
                        blocks = cfg.get("blocks").and_then(|v| v.as_integer()).map(|v| v as usize).unwrap_or(5);
                        if let Some(ratios_arr) = cfg.get("ratios").and_then(|v| v.as_array()) {
                            let parsed: Vec<String> = ratios_arr.iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect();
                            if !parsed.is_empty() {
                                ratios = parsed;
                            }
                        }
                        threshold = cfg.get("threshold").and_then(|v| v.as_integer()).map(|v| v as u32).unwrap_or(0);
                        auto_compress = cfg.get("auto_compress").and_then(|v| v.as_bool()).unwrap_or(true);
                    }
                }
            }

            let mut block_descs: Vec<String> = Vec::new();
            for (i, r) in ratios.iter().enumerate() {
                let desc = match r.as_str() {
                    "Delete" => format!("块{}: 删除", i),
                    "Full" => format!("块{}: 不压缩", i),
                    s if s.ends_with('%') => format!("块{}: 压缩到{}", i, s),
                    _ => format!("块{}: {}", i, r),
                };
                block_descs.push(desc);
            }
            let strategy_desc = format!("{}块: [{}]", blocks, block_descs.join(" | "));

            let threshold_desc = if threshold == 0 {
                "自动（模型上限/2）".to_string()
            } else {
                format!("{} tokens", threshold)
            };

            serde_json::json!({
                "策略描述": strategy_desc,
                "块数": blocks,
                "每块比例": ratios,
                "阈值": threshold,
                "阈值描述": threshold_desc,
                "自动压缩": auto_compress,
                "状态": "运行中",
                "版本": COMPRESS_VERSION,
                "配置来源": config_path.to_string_lossy().to_string(),
            })
        },
        "board" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let tremolite_dir = std::path::Path::new(&home).join(".tremolite");
            let plan_path = tremolite_dir.join("profiles").join(&state.profile_name).join("data").join("plan").join("plans.json");

            let mut total = 0u64;
            let mut draft = 0u64;
            let mut in_progress = 0u64;
            let mut completed = 0u64;
            let mut cancelled = 0u64;
            let mut plan_list: Vec<serde_json::Value> = Vec::new();

            if let Ok(content) = std::fs::read_to_string(&plan_path) {
                if let Ok(plans) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                    total = plans.len() as u64;
                    for p in &plans {
                        let status = p.get("status").and_then(|v| v.as_str()).unwrap_or("");
                        let steps = p.get("steps").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
                        let done_steps = p.get("steps").and_then(|v| v.as_array()).map(|a| {
                            a.iter().filter(|s| s.get("status").and_then(|x| x.as_str()) == Some("Completed")).count()
                        }).unwrap_or(0);
                        let progress = if steps > 0 { done_steps as f64 / steps as f64 } else { 0.0 };

                        match status {
                            "Draft" => draft += 1,
                            "InProgress" | "Approved" | "Reviewing" => in_progress += 1,
                            "Completed" => completed += 1,
                            "Cancelled" | "Archived" => cancelled += 1,
                            _ => {}
                        }

                        plan_list.push(serde_json::json!({
                            "id": p.get("id"),
                            "title": p.get("title"),
                            "description": p.get("description"),
                            "status": status,
                            "priority": p.get("priority").and_then(|v| v.as_u64()).unwrap_or(3),
                            "steps": steps,
                            "done_steps": done_steps,
                            "progress": (progress * 100.0).round() as u64,
                            "created_at": p.get("created_at"),
                            "updated_at": p.get("updated_at"),
                            "tags": p.get("tags").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
                        }));
                    }
                }
            }

            plan_list.sort_by(|a, b| {
                let pa = a.get("priority").and_then(|v| v.as_u64()).unwrap_or(0);
                let pb = b.get("priority").and_then(|v| v.as_u64()).unwrap_or(0);
                pb.cmp(&pa)
            });

            serde_json::json!({
                "stats": {
                    "total": total,
                    "draft": draft,
                    "in_progress": in_progress,
                    "completed": completed,
                    "cancelled": cancelled,
                },
                "plans": plan_list,
                "storage_path": plan_path.to_string_lossy().to_string(),
                "version": PLAN_VERSION,
            })
        },
        "delegation" => serde_json::json!({
            "最大子Agent": "3",
            "委派深度": "1",
            "活跃任务": 0,
            "版本": DELEGATION_VERSION,
        }),
        "channels" => {
            let config_str = std::fs::read_to_string(
                tremolite_dir.join("profiles").join(&state.profile_name).join("config.toml")
            ).ok().unwrap_or_default();
            let mut channel_count = 0u32;
            let mut channel_names: Vec<serde_json::Value> = Vec::new();
            let mut dash_port = 5835u16;
            for line in config_str.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("[channels.") {
                    if let Some(end) = rest.find(']') {
                        channel_count += 1;
                        let ch_name = rest[..end].to_string();
                        channel_names.push(serde_json::json!({
                            "名称": ch_name,
                            "类型": "未知",
                            "ID": "",
                        }));
                    }
                }
                if let Some(rest) = t.strip_prefix("listen = \"") {
                    if let Some(end) = rest.find('\"') {
                        if let Some(p_str) = rest[..end].split(':').last() {
                            if let Ok(p) = p_str.parse::<u16>() {
                                dash_port = p;
                            }
                        }
                    }
                }
            }
            // 从 channels_registry 补充类型和 ID
            let reg_path = tremolite_dir.join("profiles").join(&state.profile_name).join("channels_registry.json");
            if let Ok(reg_str) = std::fs::read_to_string(&reg_path) {
                if let Ok(registry) = serde_json::from_str::<Vec<serde_json::Value>>(&reg_str) {
                    for entry in &registry {
                        let entry_name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
                        let entry_type = entry.get("type").and_then(|v| v.as_str()).unwrap_or("");
                        let entry_id = entry.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        for ch in &mut channel_names {
                            let ch_name = ch.get("名称").and_then(|v| v.as_str()).unwrap_or("");
                            if ch_name == entry_name {
                                if let Some(obj) = ch.as_object_mut() {
                                    obj.insert("类型".into(), serde_json::json!(entry_type));
                                    obj.insert("ID".into(), serde_json::json!(entry_id));
                                }
                            }
                        }
                    }
                }
            }
            serde_json::json!({
                "已注册通道": channel_count,
                "通道列表": channel_names,
                "端口": dash_port,
                "状态": "运行中",
                "版本": CHANNELS_VERSION,
            })
        }
        "user" => {
            let profile_dir = tremolite_dir.join("profiles").join(&state.profile_name);
            let users_dir = profile_dir.join("users");
            let mut admins = Vec::new();
            let mut regulars = Vec::new();
            let mut total = 0u64;

            if let Ok(entries) = std::fs::read_dir(&users_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("toml") { continue; }
                    let uid = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(val) = content.parse::<toml::Value>() {
                            let alias = val.get("alias");
                            let display_name = alias.and_then(|a| a.get("display_name")).and_then(|v| v.as_str())
                                .or_else(|| val.get("display_name").and_then(|v| v.as_str()))
                                .or_else(|| alias.and_then(|a| a.get("name")).and_then(|v| v.as_str()))
                                .or_else(|| val.get("name").and_then(|v| v.as_str()))
                                .unwrap_or(&uid).to_string();
                            let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("user");
                            let qq = alias.and_then(|a| a.get("qq")).and_then(|v| v.as_str())
                                .or_else(|| val.get("qq").and_then(|v| v.as_str()))
                                .unwrap_or("");
                            let account_name = alias.and_then(|a| a.get("name")).and_then(|v| v.as_str())
                                .or_else(|| val.get("account_name").and_then(|v| v.as_str()))
                                .or_else(|| val.get("name").and_then(|v| v.as_str()))
                                .unwrap_or("");
                            let alias_others: Vec<String> = val.get("alias")
                                .and_then(|a| a.get("others"))
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                .unwrap_or_default();
                            let entry = serde_json::json!({
                                "uid": uid,
                                "display_name": display_name,
                                "role": role,
                                "qq": qq,
                                "account_name": account_name,
                                "alias_others": alias_others,
                            });
                            if role == "admin" { admins.push(entry); }
                            else { regulars.push(entry); }
                            total += 1;
                        } else {
                            // TOML 解析失败 → 行级 fallback
                            tracing::warn!("user: failed to parse TOML for {}, using line-level fallback", uid);
                            let mut fallback_role = "user".to_string();
                            let mut fallback_display = uid.clone();
                            let mut fallback_qq = String::new();
                            let mut fallback_name = String::new();
                            for line in content.lines() {
                                let t = line.trim();
                                if let Some(v) = t.strip_prefix("role = \"") {
                                    if let Some(end) = v.find('"') {
                                        fallback_role = v[..end].to_string();
                                    }
                                } else if let Some(v) = t.strip_prefix("display_name = \"") {
                                    if let Some(end) = v.find('"') {
                                        fallback_display = v[..end].to_string();
                                    }
                                } else if let Some(v) = t.strip_prefix("qq = \"") {
                                    if let Some(end) = v.find('"') {
                                        fallback_qq = v[..end].to_string();
                                    }
                                } else if let Some(v) = t.strip_prefix("name = \"") {
                                    if let Some(end) = v.find('"') {
                                        fallback_name = v[..end].to_string();
                                    }
                                }
                            }
                            let entry = serde_json::json!({
                                "uid": uid,
                                "display_name": fallback_display,
                                "role": fallback_role,
                                "qq": fallback_qq,
                                "account_name": fallback_name,
                                "alias_others": serde_json::Value::Array(vec![]),
                            });
                            if fallback_role == "admin" { admins.push(entry); }
                            else { regulars.push(entry); }
                            total += 1;
                        }
                    }
                }
            }

            // 获取 UserModule 的上次注入内容
            let user_injected = state.modules.as_ref()
                .and_then(|mods| mods.with_module("user", |m| {
                    m.as_any()
                        .and_then(|any| any.downcast_ref::<tremolite_core::UserModule>())
                        .map(|um| um.last_injected.clone())
                }))
                .flatten()
                .unwrap_or_default();

            serde_json::json!({
                "注册用户数": total,
                "管理员账户": admins.len(),
                "普通用户": regulars.len(),
                "管理员列表": admins,
                "普通用户列表": regulars,
                "配置包": users_dir.to_string_lossy().to_string(),
                "版本": tremolite_core::CORE_VERSION,
                "最后注入内容": user_injected,
            })
        }
        "mcp" => {
            let mcp_path = tremolite_dir.join("data").join("mcp_discovered.json");
            let mcp_data: serde_json::Value = std::fs::read_to_string(&mcp_path).ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default();
            let servers: Vec<serde_json::Value> = mcp_data.as_object()
                .map(|obj| {
                    obj.iter().map(|(name, info)| {
                        let tools = info.get("tools").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        let resources = info.get("resources").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        let prompts = info.get("prompts").and_then(|v| v.as_array()).cloned().unwrap_or_default();
                        serde_json::json!({
                            "name": name,
                            "transport": "stdio",
                            "status": "connected",
                            "tools": tools,
                            "resources": resources,
                            "prompts": prompts,
                        })
                    }).collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let total_tools: usize = servers.iter().map(|s| {
                s.get("tools").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
            }).sum();
            let total_resources: usize = servers.iter().map(|s| {
                s.get("resources").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
            }).sum();
            let total_prompts: usize = servers.iter().map(|s| {
                s.get("prompts").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0)
            }).sum();
            serde_json::json!({
                "servers": servers,
                "server_count": servers.len(),
                "total_tools": total_tools,
                "total_resources": total_resources,
                "total_prompts": total_prompts,
                "模块ID": "mcp",
                "状态": "已连接",
            })
        }
        _ => serde_json::json!({
            "模块ID": mod_id,
            "状态": "未知模块",
        }),
    };

    Json(serde_json::json!({
        "status": "ok",
        "模块ID": mod_id,
        "data": data,
    }))
}

async fn handle_delegation_settings_save(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("modules").join("delegation.toml");

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let max_sub = payload.get("max_sub_agents").and_then(|v| v.as_u64()).unwrap_or(3);
    let max_depth = payload.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(1);
    let timeout = payload.get("default_timeout").and_then(|v| v.as_u64()).unwrap_or(300);

    let content = format!(
        "[delegation]\nmax_sub_agents = {}\nmax_depth = {}\ndefault_timeout = {}\n",
        max_sub, max_depth, timeout
    );

    match std::fs::write(&config_path, &content) {
        Ok(_) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("保存失败: {}", e)})),
    }
}

async fn handle_delegation_settings_get(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("modules").join("delegation.toml");

    let content = std::fs::read_to_string(&config_path).unwrap_or_default();
    let max_sub = 3u64;
    let max_depth = 1u64;
    let timeout = 300u64;

    let parsed: toml::Value = content.parse().unwrap_or(toml::Value::Table(toml::map::Map::new()));
    let del = parsed.get("delegation");
    let max_sub = del.and_then(|d| d.get("max_sub_agents")).and_then(|v| v.as_integer()).unwrap_or(3) as u64;
    let max_depth = del.and_then(|d| d.get("max_depth")).and_then(|v| v.as_integer()).unwrap_or(1) as u64;
    let timeout = del.and_then(|d| d.get("default_timeout")).and_then(|v| v.as_integer()).unwrap_or(300) as u64;

    Json(serde_json::json!({
        "status": "ok",
        "data": {
            "max_sub_agents": max_sub,
            "max_depth": max_depth,
            "default_timeout": timeout,
        }
    }))
}

async fn handle_acp_agent_add(
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

async fn handle_acp_agent_remove(
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"status": "ok"}))
}

fn toml_to_json(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(a) => serde_json::Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => {
            let mut map = serde_json::Map::new();
            for (k, v) in t {
                map.insert(k.clone(), toml_to_json(v));
            }
            serde_json::Value::Object(map)
        }
    }
}

async fn handle_mcp_servers() -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mcp_path = std::path::Path::new(&home)
        .join(".tremolite").join("data").join("mcp_discovered.json");
    let mcp_data: serde_json::Value = std::fs::read_to_string(&mcp_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mcp_config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join("aoi").join("modules").join("mcp.toml");
    let config_str = std::fs::read_to_string(&mcp_config_path).ok().unwrap_or_default();
    let toml_val: toml::Value = config_str.parse().unwrap_or(toml::Value::Table(toml::map::Map::new()));
    let servers_cfg = toml_val.get("mcp")
        .and_then(|m| m.get("servers"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut servers = Vec::new();
    for cfg in &servers_cfg {
        let name = cfg.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let discovered = mcp_data.get(name);
        let tools = discovered.and_then(|d| d.get("tools")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let resources = discovered.and_then(|d| d.get("resources")).and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let prompts = discovered.and_then(|d| d.get("prompts")).and_then(|v| v.as_array()).cloned().unwrap_or_default();

        let transport = cfg.get("transport").or(cfg.get("transport_type"))
            .map(toml_to_json)
            .unwrap_or(serde_json::Value::String("stdio".into()));
        let prefix = cfg.get("prefix")
            .map(toml_to_json)
            .unwrap_or(serde_json::Value::String("".into()));
        let timeout = cfg.get("timeout_secs")
            .map(toml_to_json)
            .unwrap_or(serde_json::Value::Number(serde_json::Number::from(30)));

        servers.push(serde_json::json!({
            "name": cfg.get("name").map(toml_to_json),
            "transport": transport,
            "url": cfg.get("url").map(toml_to_json),
            "command": cfg.get("command").map(toml_to_json),
            "args": cfg.get("args").map(toml_to_json),
            "prefix": prefix,
            "timeout_secs": timeout,
            "env": cfg.get("env").map(toml_to_json),
            "tools": tools,
            "resources": resources,
            "prompts": prompts,
        }));
    }

    Json(serde_json::json!({
        "status": "ok",
        "servers": servers
    }))
}

// ─── Handler: 模块开关 ────────────────────
async fn handle_module_toggle(
    axum::extract::Path(mod_id): Path<String>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let state_path = std::path::Path::new(&home)
        .join(".tremolite").join("data").join("tremolite").join("modules_state.json");
    // 读当前状态
    let mut states = std::fs::read_to_string(&state_path).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(serde_json::json!({}));
    let enabled = payload.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    // 确保是对象
    if let Some(obj) = states.as_object_mut() {
        obj.insert(mod_id.clone(), serde_json::json!(enabled));
    } else {
        let mut map = serde_json::Map::new();
        map.insert(mod_id.clone(), serde_json::json!(enabled));
        states = serde_json::json!(map);
    }
    // 写回
    if let Some(parent) = state_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&state_path, serde_json::to_string_pretty(&states).unwrap_or_default());
    Json(serde_json::json!({
        "status": "ok",
        "模块ID": mod_id,
        "enabled": enabled,
    }))
}

// ─── Handler: 重启透闪石 ──────────────────────────
// 调整模块开关后，用户点此按钮重启进程使变更生效
async fn handle_restart() -> Json<serde_json::Value> {
    // 先回复 success，然后后台等一秒再杀进程（watchdog 会自动拉起）
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let _ = std::process::Command::new("pkill")
            .arg("-f")
            .arg("tremolite-cli daemon")
            .spawn();
    });
    Json(serde_json::json!({
        "status": "ok",
        "message": "正在重启透闪石……watchdog 会在几秒内重新拉起。",
    }))
}

// ─── Helper: 从 Plutchik 值推导风格 ──────────────
fn compute_emotion_style(plutchik: &serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    // 找主导情绪
    let dominant = plutchik.iter()
        .max_by(|a, b| a.1.as_f64().unwrap_or(0.0).partial_cmp(&b.1.as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(k, _)| k.as_str())
        .unwrap_or("trust");

    // 从 EmotionState 计算复合标签 + 强度
    let mut state = tremolite_emotion::EmotionState::new();
    if let Some(v) = plutchik.get("joy").and_then(|x| x.as_f64()) { state.joy = v; }
    if let Some(v) = plutchik.get("sadness").and_then(|x| x.as_f64()) { state.sadness = v; }
    if let Some(v) = plutchik.get("anger").and_then(|x| x.as_f64()) { state.anger = v; }
    if let Some(v) = plutchik.get("fear").and_then(|x| x.as_f64()) { state.fear = v; }
    if let Some(v) = plutchik.get("surprise").and_then(|x| x.as_f64()) { state.surprise = v; }
    if let Some(v) = plutchik.get("disgust").and_then(|x| x.as_f64()) { state.disgust = v; }
    if let Some(v) = plutchik.get("anticipation").and_then(|x| x.as_f64()) { state.anticipation = v; }
    if let Some(v) = plutchik.get("trust").and_then(|x| x.as_f64()) { state.trust = v; }
    let result = state.emotion_result();
    let composite = result.label.clone();

    // 用 ToneMap 读风格数据（injection 由 get_injection 动态生成）
    let home = std::env::var("HOME").unwrap_or_default();
    let tone_path = std::path::Path::new(&home).join(".tremolite").join("data").join("tone_map.json");
    let tone_map = tremolite_emotion::ToneMap::load(&tone_path.to_string_lossy());
    let injection = tone_map.get_injection(&result).unwrap_or_default();

    // 从 tone_map 原始数据提取 emoji/style_label/example
    let mut style_label = dominant.to_string();
    let mut emoji = String::new();
    let mut example = String::new();
    if let Some(entry) = tone_map.entries.get(&composite) {
        if let Some(level) = entry.levels.get(result.intensity.as_str()) {
            style_label = level.style.clone();
            if let Some(e) = &level.emoji { emoji = e.clone(); }
            if let Some(tpl) = &level.模板 {
                if let Some(e) = tpl.句式示例.first() { example = e.clone(); }
            }
        }
    }

    serde_json::json!({
        "dominant": dominant,
        "style_label": style_label,
        "composite": composite,
        "example": example,
        "emoji": emoji,
        "injection": injection,
    })
}

fn emotion_path(state: &AppState) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".tremolite").join("profiles").join(&state.profile_name).join("emotion.json")
}

fn emotion_history_path(state: &AppState) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".tremolite").join("profiles").join(&state.profile_name).join("emotion_history.json")
}

// ─── Handler: 情绪状态 + 历史 ──────────────────────
async fn handle_emotion_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let path = emotion_path(&state);
    let hist_path = emotion_history_path(&state);

    // 读当前情绪
    let emotion_data = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or_default();
    let plutchik = emotion_data.get("plutchik").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let energy = emotion_data.get("energy").and_then(|v| v.as_f64()).unwrap_or(50.0);

    // 读历史
    let history: Vec<serde_json::Value> = std::fs::read_to_string(&hist_path).ok()
        .and_then(|s| serde_json::from_str::<Vec<serde_json::Value>>(&s).ok())
        .unwrap_or_default();

    // 风格
    let style = compute_emotion_style(&plutchik);
    // 读自动波动间隔
    let interval = emotion_data.get("auto_fluctuation_seconds").and_then(|v| v.as_f64()).unwrap_or(1800.0);

    Json(serde_json::json!({
        "status": "ok",
        "data": {
            "current": {
                "plutchik": plutchik,
                "energy": energy,
            },
            "style": style,
            "history": history,
            "auto_fluctuation_seconds": interval,
            "version": MCP_VERSION,
        }
    }))
}

// ─── Handler: 更新情绪维度 ──────────────────────
async fn handle_emotion_update(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let path = emotion_path(&state);
    let hist_path = emotion_history_path(&state);

    let dimension = payload.get("dimension").and_then(|v| v.as_str()).unwrap_or("");
    let value = payload.get("value").and_then(|v| v.as_f64()).unwrap_or(50.0);
    if dimension.is_empty() {
        return Json(serde_json::json!({"status": "error", "error": "缺少 dimension"}));
    }
    let valid_dims = ["joy","sadness","anger","fear","surprise","disgust","anticipation","trust","energy"];
    if !valid_dims.contains(&dimension) {
        return Json(serde_json::json!({"status": "error", "error": format!("无效维度: {dimension}")}));
    }

    let mut data: serde_json::Value = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    if dimension == "energy" {
        if let Some(obj) = data.as_object_mut() {
            obj.insert("energy".into(), serde_json::json!(value.max(0.0).min(100.0)));
        }
    } else {
        if let Some(plutchik) = data.get_mut("plutchik").and_then(|v| v.as_object_mut()) {
            plutchik.insert(dimension.into(), serde_json::json!(value.max(0.0).min(100.0)));
        }
    }
    if let Some(obj) = data.as_object_mut() {
        obj.insert("last_update".into(), serde_json::json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().to_string()));
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap_or_default());

    // 记录历史
    let new_plutchik = data.get("plutchik").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let style = compute_emotion_style(&new_plutchik);
    let entry = serde_json::json!({
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "type": "manual",
        "dimension": dimension,
        "value": value,
        "plutchik": new_plutchik,
        "style": style.get("style_label").and_then(|v| v.as_str()).unwrap_or(""),
    });
    let mut history: Vec<serde_json::Value> = std::fs::read_to_string(&hist_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    history.push(entry);
    if history.len() > 100 { history.drain(0..history.len()-100); }
    let _ = std::fs::write(&hist_path, serde_json::to_string_pretty(&history).unwrap_or_default());

    Json(serde_json::json!({"status": "ok"}))
}

// ─── Handler: 立即情绪波动 ──────────────────────
async fn handle_emotion_fluctuate(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let path = emotion_path(&state);
    let hist_path = emotion_history_path(&state);

    let mut data: serde_json::Value = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    // 给八维随机增减 5-20
    let dims = ["joy","sadness","anger","fear","surprise","disgust","anticipation","trust"];
    if let Some(plutchik) = data.get_mut("plutchik").and_then(|v| v.as_object_mut()) {
        for d in &dims {
            let old = plutchik.get(*d).and_then(|v| v.as_f64()).unwrap_or(50.0);
            let delta = rand::random::<f64>() * 30.0 - 15.0 ; // -15 ~ +15
            let new = (old + delta).clamp(0.0, 100.0);
            plutchik.insert(d.to_string(), serde_json::json!(new));
        }
    }
    if let Some(obj) = data.as_object_mut() {
        obj.insert("last_fluctuation".into(), serde_json::json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().to_string()));
        obj.insert("last_update".into(), serde_json::json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs().to_string()));
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap_or_default());

    // 记录历史
    let new_plutchik = data.get("plutchik").and_then(|v| v.as_object()).cloned().unwrap_or_default();
    let style = compute_emotion_style(&new_plutchik);
    let entry = serde_json::json!({
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
        "type": "manual_fluctuation",
        "plutchik": new_plutchik,
        "style": style.get("style_label").and_then(|v| v.as_str()).unwrap_or(""),
    });
    let mut history: Vec<serde_json::Value> = std::fs::read_to_string(&hist_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    history.push(entry);
    if history.len() > 100 { history.drain(0..history.len()-100); }
    let _ = std::fs::write(&hist_path, serde_json::to_string_pretty(&history).unwrap_or_default());

    Json(serde_json::json!({"status": "ok"}))
}

// ─── Handler: 读取自动波动间隔 ──────────────────
async fn handle_emotion_interval_get(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let path = emotion_path(&state);

    let data: serde_json::Value = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let interval = data.get("auto_fluctuation_seconds").and_then(|v| v.as_f64()).unwrap_or(1800.0);

    Json(serde_json::json!({
        "status": "ok",
        "auto_fluctuation_seconds": interval,
    }))
}

// ─── Handler: 设置自动波动间隔 ──────────────────
async fn handle_emotion_interval_set(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let path = emotion_path(&state);

    let seconds = payload.get("seconds").and_then(|v| v.as_f64()).unwrap_or(1800.0);
    let seconds = seconds.max(10.0).min(3600.0);

    let mut data: serde_json::Value = std::fs::read_to_string(&path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    if let Some(obj) = data.as_object_mut() {
        obj.insert("auto_fluctuation_seconds".into(), serde_json::json!(seconds));
    }
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap_or_default());

    Json(serde_json::json!({
        "status": "ok",
        "auto_fluctuation_seconds": seconds,
    }))
}

// ─── Handler: 定时任务 (Cron) 管理 ─────────────────
fn cron_path(state: &AppState) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&home).join(".tremolite").join("profiles").join(&state.profile_name).join("cron_tasks.json")
}
fn cron_load(state: &AppState) -> Vec<serde_json::Value> {
    std::fs::read_to_string(cron_path(state))
        .ok().and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
fn cron_save(state: &AppState, tasks: &[serde_json::Value]) {
    let _ = std::fs::write(cron_path(state), serde_json::to_string_pretty(tasks).unwrap_or_default());
}

async fn handle_channel_targets(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let reg_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name).join("channels_registry.json");
    let channels: Vec<serde_json::Value> = std::fs::read_to_string(&reg_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut options: Vec<serde_json::Value> = Vec::new();

    // 内建调度语义——和通道并列的选项
    options.push(serde_json::json!({"value": "origin", "label": "origin — 投回当前对话", "targets": []}));
    options.push(serde_json::json!({"value": "all",    "label": "all — 广播所有通道",    "targets": []}));
    options.push(serde_json::json!({"value": "local",  "label": "local — 仅存本地",      "targets": []}));

    // 从 channels_registry.json 读取实际消息通道，id 自动编号为 ch_序号（从1开始）
    for (i, ch) in channels.iter().enumerate() {
        let id = format!("ch_{:02}", i + 1);
        let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let ch_type = ch.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let label = format!("{} · {}（{}）", id, name, ch_type);
        options.push(serde_json::json!({
            "value": name,
            "label": label,
            "targets": []
        }));
    }

    Json(serde_json::json!({"status":"ok", "options": options}))
}

async fn handle_channels_manage(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let reg_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name).join("channels_registry.json");
    let data: Vec<serde_json::Value> = std::fs::read_to_string(&reg_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    Json(serde_json::json!({"status":"ok","data":data,"version":CHANNELS_VERSION}))
}

async fn handle_channels_save(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let reg_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name).join("channels_registry.json");
    if let Some(parent) = reg_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let channels = payload.get("channels").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let _ = std::fs::write(&reg_path, serde_json::to_string_pretty(&channels).unwrap_or_default());
    Json(serde_json::json!({"status":"ok"}))
}

async fn handle_channels_sync_config(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_dir = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name);
    let reg_path = profile_dir.join("channels_registry.json");
    let config_path = profile_dir.join("config.toml");

    let channels: Vec<serde_json::Value> = std::fs::read_to_string(&reg_path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();

    let mut channels_toml = String::new();
    for ch in &channels {
        let name = ch.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
        let ch_type = ch.get("type").and_then(|v| v.as_str()).unwrap_or("");
        channels_toml.push_str(&format!("\n[channels.{}]\n", name));
        match ch_type {
            "QqBot" => {
                let app_id = ch.get("app_id").and_then(|v| v.as_str()).unwrap_or("");
                let client_secret = ch.get("client_secret").and_then(|v| v.as_str()).unwrap_or("");
                let token = ch.get("token").and_then(|v| v.as_str()).unwrap_or("");
                let production = ch.get("production").and_then(|v| v.as_bool()).unwrap_or(false);
                let broadcast_target = ch.get("broadcast_target").and_then(|v| v.as_str()).unwrap_or("");
                channels_toml.push_str(&format!("type = \"QqBot\"\napp_id = \"{}\"\nclient_secret = \"{}\"\ntoken = \"{}\"\nproduction = {}\n", app_id, client_secret, token, production));
                if !broadcast_target.is_empty() {
                    channels_toml.push_str(&format!("broadcast_target = \"{}\"\n", broadcast_target));
                }
            }
            _ => {
                channels_toml.push_str(&format!("type = \"{}\"\n", ch_type));
                for (k, v) in ch.as_object().unwrap_or(&serde_json::Map::new()) {
                    if k == "name" || k == "type" || k == "id" { continue; }
                    if let Some(s) = v.as_str() {
                        channels_toml.push_str(&format!("{} = \"{}\"\n", k, s));
                    } else if let Some(b) = v.as_bool() {
                        channels_toml.push_str(&format!("{} = {}\n", k, b));
                    } else if let Some(n) = v.as_f64() {
                        channels_toml.push_str(&format!("{} = {}\n", k, n));
                    }
                }
            }
        }
    }

    let existing_config = std::fs::read_to_string(&config_path).unwrap_or_default();
    let lines: Vec<&str> = existing_config.lines().collect();
    // 移除旧的 [channels.*] 段
    let mut keep = Vec::new();
    let mut in_channels = false;
    for line in &lines {
        if line.trim().starts_with("[channels.") {
            in_channels = true;
            continue;
        }
        if in_channels {
            if line.trim().starts_with('[') && !line.trim().starts_with("[channels.") {
                in_channels = false;
                keep.push(*line);
            }
            continue;
        }
        keep.push(*line);
    }
    let cleaned = keep.join("\n");
    let new_config = if channels_toml.is_empty() {
        cleaned
    } else {
        format!("{}{}", cleaned, channels_toml)
    };
    let _ = std::fs::write(&config_path, &new_config);

    Json(serde_json::json!({
        "status": "ok",
        "message": "已同步到配置，重启生效",
    }))
}

async fn handle_cron_tasks(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({"status":"ok","tasks":cron_load(&state)}))
}
async fn handle_cron_create(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("未命名").to_string();
    let schedule = payload.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 */30 * * * *").to_string();
    let command = payload.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let deliver = payload.get("deliver").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let cmd_type = payload.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let mut list = cron_load(&state);
    let mut entry = serde_json::json!({
        "name": name,
        "schedule": schedule,
        "command": command,
        "deliver": deliver,
        "enabled": true,
        "last_run": 0,
        "created_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(),
    });
    if !cmd_type.is_empty() {
        entry["type"] = serde_json::json!(cmd_type);
    }
    list.push(entry);
    cron_save(&state, &list);
    Json(serde_json::json!({"status":"ok"}))
}
async fn handle_cron_toggle(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Path(idx): axum::extract::Path<usize>,
) -> Json<serde_json::Value> {
    let mut list = cron_load(&state);
    if let Some(obj) = list.get_mut(idx).and_then(|v| v.as_object_mut()) {
        let en = obj.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
        obj.insert("enabled".into(), serde_json::json!(!en));
        cron_save(&state, &list);
        Json(serde_json::json!({"status":"ok"}))
    } else {
        Json(serde_json::json!({"status":"error","error":"索引超出范围"}))
    }
}
async fn handle_cron_delete(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Path(idx): axum::extract::Path<usize>,
) -> Json<serde_json::Value> {
    let mut list = cron_load(&state);
    if idx < list.len() {
        list.remove(idx);
        cron_save(&state, &list);
    }
    Json(serde_json::json!({"status":"ok"}))
}

fn normalize_deliver(d: &str) -> &str {
    match d {
        "" | "all" | "broadcast" | "everywhere" => "__all__",
        _ => {
            if let Some(pos) = d.find(':') {
                &d[..pos]
            } else {
                d
            }
        }
    }
}

#[allow(dead_code)]
fn extract_deliver_target(d: &str) -> Option<String> {
    let pos = d.find(':')?;
    let rest = &d[pos + 1..];
    if rest.is_empty() { None } else { Some(rest.to_string()) }
}

fn parse_schedule_str(s: &str) -> Schedule {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 5 {
        if parts.len() >= 6 && parts[0] == "0" && parts[1].starts_with("*/") {
            let n: u64 = parts[1].trim_start_matches("*/").parse().unwrap_or(30);
            return Schedule::EverySecs(n * 60);
        }
        if parts.len() >= 6 && parts[0] == "0" && parts[1] == "0" && parts[2].starts_with("*/") {
            let n: u64 = parts[2].trim_start_matches("*/").parse().unwrap_or(1);
            return Schedule::EverySecs(n * 3600);
        }
        if parts.len() >= 6 && parts[3] == "*" && parts[4] == "*" && parts[5] == "*" {
            let hour: u8 = parts[2].parse().unwrap_or(0);
            let minute: u8 = parts[1].parse().unwrap_or(0);
            return Schedule::Daily { hour, minute };
        }
    }
    let five: String = parts.iter().take(5).cloned().collect::<Vec<_>>().join(" ");
    Schedule::CronExpr(five)
}

async fn handle_cron_run(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Path(idx): axum::extract::Path<usize>,
) -> Json<serde_json::Value> {
    let mut list = cron_load(&state);
    let entry = match list.get_mut(idx) {
        Some(v) => v,
        None => return Json(serde_json::json!({"status":"error","error":"索引超出范围"})),
    };
    let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("未命名").to_string();
    let command = entry.get("command").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let deliver_raw = entry.get("deliver").and_then(|v| v.as_str()).unwrap_or("origin");
    let channel = normalize_deliver(deliver_raw).to_string();
    let is_prompt = entry.get("type").and_then(|v| v.as_str()) == Some("prompt");
    let sched_str = entry.get("schedule").and_then(|v| v.as_str()).unwrap_or("0 */30 * * * *");

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let sched = parse_schedule_str(sched_str);
    let next_run = tremolite_cron::calc_next_run_at(&sched, now_secs);
    if let Some(obj) = entry.as_object_mut() {
        obj.insert("last_run".into(), serde_json::json!(now_secs));
        obj.insert("next_run".into(), serde_json::json!(next_run));
    }
    cron_save(&state, &list);

    if is_prompt {
        let task = SessionTask {
            session_id: format!("cron-{}", name),
            input: command,
            channel,
            sender: "manual-run".to_string(),
        };
        if state.inbound_tx.send(task).is_err() {
            return Json(serde_json::json!({"status":"error","error":"调度器不可用"}));
        }
        Json(serde_json::json!({"status":"ok","message":"已触发立即执行"}))
    } else {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .output();
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let stderr = String::from_utf8_lossy(&out.stderr);
                if !stdout.trim().is_empty() {
                    let task = SessionTask {
                        session_id: format!("cron-{}", name),
                        input: stdout.trim().to_string(),
                        channel,
                        sender: format!("cron-{}", name),
                    };
                    let _ = state.inbound_tx.send(task);
                }
                let mut msg = format!("shell 任务 '{}' 已执行 (exit: {})", name, out.status);
                if !stdout.is_empty() {
                    msg.push_str(&format!("\nstdout:\n{}", stdout.trim()));
                }
                if !stderr.is_empty() {
                    msg.push_str(&format!("\nstderr:\n{}", stderr.trim()));
                }
                Json(serde_json::json!({"status":"ok","message": msg}))
            }
            Err(e) => {
                Json(serde_json::json!({"status":"error","error": format!("执行失败: {}", e)}))
            }
        }
    }
}

async fn handle_cron_update(
    Extension(state): Extension<Arc<AppState>>,
    axum::extract::Path(idx): axum::extract::Path<usize>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let mut list = cron_load(&state);
    if idx < list.len() {
        if let Some(obj) = list[idx].as_object_mut() {
            if let Some(name) = payload.get("name").and_then(|v| v.as_str()) {
                obj.insert("name".into(), serde_json::json!(name));
            }
            if let Some(schedule) = payload.get("schedule").and_then(|v| v.as_str()) {
                obj.insert("schedule".into(), serde_json::json!(schedule));
            }
            if let Some(command) = payload.get("command").and_then(|v| v.as_str()) {
                obj.insert("command".into(), serde_json::json!(command));
            }
            if let Some(deliver) = payload.get("deliver").and_then(|v| v.as_str()) {
                obj.insert("deliver".into(), serde_json::json!(deliver));
            }
            if let Some(cmd_type) = payload.get("type").and_then(|v| v.as_str()) {
                if cmd_type.is_empty() {
                    obj.remove("type");
                } else {
                    obj.insert("type".into(), serde_json::json!(cmd_type));
                }
            }
        }
        cron_save(&state, &list);
    }
    Json(serde_json::json!({"status":"ok"}))
}

// ─── Handler: 配置包详情数据 ────────────────────
/// 查询参数: ?name=xxx
async fn handle_profile_list(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let name = params.get("name").map(|s| s.as_str()).unwrap_or("");
    if name.is_empty() {
        return Json(serde_json::json!({"status": "error", "error": "缺少包名"}));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let profiles_base = std::path::Path::new(&home).join(".tremolite").join("profiles");
    let profile_dir = profiles_base.join(name);
    if !profile_dir.is_dir() {
        return Json(serde_json::json!({"status": "error", "error": format!("包 '{name}' 不存在")}));
    }

    let mut files: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    if let Ok(entries) = walk_dir(&profile_dir, &profile_dir) {
        for (rel, content) in entries {
            files.insert(rel, content);
        }
    }

    Json(serde_json::json!({
        "status": "ok",
        "name": name,
        "files": files,
    }))
}

fn walk_dir(base: &std::path::Path, dir: &std::path::Path) -> std::io::Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let rel = path.strip_prefix(base).unwrap_or(&path)
            .to_string_lossy().to_string();
        if entry.file_type()?.is_dir() {
            result.extend(walk_dir(base, &path)?);
        } else {
            // 跳过二进制大文件（tone_map.json 太大，只显示前100行）
            let fname = path.file_name().unwrap_or_default().to_string_lossy();
            if fname == "tone_map.json" {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let lines: Vec<&str> = content.lines().take(50).collect();
                    result.push((format!("{rel} (前50行/共{}行)", content.lines().count()),
                        lines.join("\n") + "\n..."));
                }
            } else if fname.ends_with(".json") || fname.ends_with(".toml") || fname.ends_with(".md") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    result.push((rel.clone(), content));
                }
            } else if fname.ends_with(".gitkeep") {
                // skip
            } else {
                result.push((rel.clone(), format!("[二进制文件: {} 字节]", 
                    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0))));
            }
        }
    }
    Ok(result)
}

// ─── Handler: 加载配置包 ───────────────────────

async fn handle_profile_load(
    axum::extract::Extension(_state): axum::extract::Extension<Arc<AppState>>,
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let name = body.get("name").and_then(|v| v.as_str()).unwrap_or("");
    if name.is_empty() {
        return Json(serde_json::json!({"status": "error", "error": "缺少包名"}));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let profiles_base = std::path::Path::new(&home).join(".tremolite").join("profiles");
    let src = profiles_base.join(name);
    if !src.is_dir() {
        return Json(serde_json::json!({"status": "error", "error": format!("包 '{name}' 不存在")}));
    }
    // 更新 config.toml 的 [profile] 段——指向新包
    let config_path = std::path::Path::new(&home).join(".tremolite").join("config.toml");
    match std::fs::read_to_string(&config_path) {
        Ok(content) => {
            let mut has_profile = false;
            let new_content = content.lines().map(|line| -> String {
                if line.trim_start().starts_with("[profile]") {
                    has_profile = true;
                    line.to_string()
                } else if has_profile && line.trim_start().starts_with("name =") {
                    has_profile = false;
                    format!("name = \"{}\"", name)
                } else {
                    line.to_string()
                }
            }).collect::<Vec<_>>().join("\n");
            let final_content = if content.contains("[profile]") {
                new_content
            } else {
                format!("[profile]\nname = \"{}\"\n\n{}", name, content)
            };
            match std::fs::write(&config_path, &final_content) {
                Ok(_) => Json(serde_json::json!({"status": "ok", "message": format!("已切换到配置包 '{}'，重启透闪石生效", name)})),
                Err(e) => Json(serde_json::json!({"status": "error", "error": format!("写入配置失败: {e}")})),
            }
        }
        Err(e) => Json(serde_json::json!({"status": "error", "error": format!("读取配置失败: {e}")})),
    }
}

fn _copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    if dst.exists() {
        std::fs::remove_dir_all(dst)?;
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            _copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// ─── Handler: 用户配置 ────────────────────────────

async fn handle_profile_get() -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_file = std::path::Path::new(&home)
        .join(".tremolite/profiles/aoi/profile.json");
    let data = if profile_file.exists() {
        std::fs::read_to_string(&profile_file).ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or(serde_json::json!({
                "username": "琳玲",
                "ai_name": "葵",
                "user_avatar": "",
                "ai_avatar": "",
            }))
    } else {
        serde_json::json!({
            "username": "琳玲",
            "ai_name": "葵",
            "user_avatar": "",
            "ai_avatar": "",
        })
    };
    Json(serde_json::json!({"status": "ok", "data": data, "path": profile_file.to_string_lossy().to_string()}))
}

async fn handle_profile_set(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_dir = std::path::Path::new(&home).join(".tremolite/profiles/aoi");
    std::fs::create_dir_all(&profile_dir).unwrap_or(());
    let profile_file = profile_dir.join("profile.json");

    let data = serde_json::json!({
        "username": body.get("username").and_then(|v| v.as_str()).unwrap_or("琳玲"),
        "ai_name": body.get("ai_name").and_then(|v| v.as_str()).unwrap_or("葵"),
        "user_avatar": body.get("user_avatar").and_then(|v| v.as_str()).unwrap_or(""),
        "ai_avatar": body.get("ai_avatar").and_then(|v| v.as_str()).unwrap_or(""),
    });
    let _ = std::fs::write(&profile_file, serde_json::to_string_pretty(&data).unwrap_or_default());
    Json(serde_json::json!({"status": "ok", "path": profile_file.to_string_lossy().to_string()}))
}

async fn handle_avatar_upload(
    mut multipart: axum::extract::Multipart,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let avatars_dir = std::path::Path::new(&home).join(".tremolite/profiles/aoi/avatars");
    let _ = std::fs::create_dir_all(&avatars_dir);

    let mut avatar_type = String::new();
    let mut file_data: Vec<u8> = Vec::new();
    let mut original_name = String::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "type" {
            avatar_type = field.text().await.unwrap_or_default();
        } else if name == "file" {
            original_name = field.file_name().unwrap_or("image.png").to_string();
            file_data = field.bytes().await.unwrap_or_default().to_vec();
        }
    }

    if file_data.is_empty() || avatar_type.is_empty() {
        return Json(serde_json::json!({"status": "error", "error": "缺少文件或类型"}));
    }

    let ext = if original_name.contains('.') {
        original_name.rsplit('.').next().unwrap_or("png").to_string()
    } else {
        "png".to_string()
    };

    let filename = format!("{}.{}", avatar_type, ext);
    let filepath = avatars_dir.join(&filename);
    if let Err(e) = std::fs::write(&filepath, &file_data) {
        return Json(serde_json::json!({"status": "error", "error": format!("写入失败: {}", e)}));
    }

    let avatar_url = format!("/avatars/{}", filename);

    Json(serde_json::json!({
        "status": "ok",
        "avatar_url": avatar_url,
        "filepath": filepath.to_string_lossy().to_string(),
    }))
}

async fn handle_avatar_serve(
    Path(filename): Path<String>,
) -> Result<(axum::http::StatusCode, [(axum::http::HeaderName, String); 2], Vec<u8>), (axum::http::StatusCode, &'static str)> {
    let home = std::env::var("HOME").unwrap_or_default();
    let filepath = std::path::Path::new(&home)
        .join(".tremolite/profiles/aoi/avatars")
        .join(&filename);

    if filename.contains("..") || filename.contains('/') {
        return Err((axum::http::StatusCode::FORBIDDEN, "forbidden"));
    }

    match std::fs::read(&filepath) {
        Ok(data) => {
            let content_type = if filename.ends_with(".png") {
                "image/png"
            } else if filename.ends_with(".jpg") || filename.ends_with(".jpeg") {
                "image/jpeg"
            } else if filename.ends_with(".gif") {
                "image/gif"
            } else if filename.ends_with(".webp") {
                "image/webp"
            } else if filename.ends_with(".svg") {
                "image/svg+xml"
            } else {
                "application/octet-stream"
            };
            Ok((
                axum::http::StatusCode::OK,
                [
                    (axum::http::HeaderName::from_static("content-type"), content_type.to_string()),
                    (axum::http::HeaderName::from_static("cache-control"), "no-cache, no-store, must-revalidate".to_string()),
                ],
                data,
            ))
        }
        Err(_) => Err((axum::http::StatusCode::NOT_FOUND, "not found")),
    }
}

// ─── Plan Handlers ──────────────────────────────

async fn handle_plan_list(
    Extension(_state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let plans = read_plans(&home);
    let mut data: Vec<serde_json::Value> = Vec::new();
    for p in &plans {
        let steps = p.get("steps").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
        let steps_done = p.get("steps").and_then(|v| v.as_array()).map(|a| {
            a.iter().filter(|s| s.get("status").and_then(|x| x.as_str()) == Some("completed")).count()
        }).unwrap_or(0);
        let progress = if steps > 0 { steps_done as f64 / steps as f64 } else { 0.0 };
        data.push(serde_json::json!({
            "id": p.get("id"),
            "title": p.get("title"),
            "description": p.get("description"),
            "status": p.get("status"),
            "progress": (progress * 100.0).round() / 100.0,
            "is_active": p.get("is_active").and_then(|v| v.as_bool()).unwrap_or(false),
            "priority": p.get("priority").and_then(|v| v.as_u64()).unwrap_or(3),
            "tags": p.get("tags").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
            "steps_total": steps,
            "steps_done": steps_done,
            "created_at": p.get("created_at"),
        }));
    }
    Json(serde_json::json!({"status": "ok", "data": data}))
}

async fn handle_plan_get(
    Extension(_state): Extension<Arc<AppState>>,
    axum::extract::Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let plans = read_plans(&home);
    let plan_id: u64 = id.parse().unwrap_or(0);
    let plan = plans.iter().find(|p| p.get("id").and_then(|v| v.as_u64()).unwrap_or(0) == plan_id);
    match plan {
        Some(p) => {
            let steps = p.get("steps").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let steps_done = p.get("steps").and_then(|v| v.as_array()).map(|a| {
                a.iter().filter(|s| s.get("status").and_then(|x| x.as_str()) == Some("completed")).count()
            }).unwrap_or(0);
            let progress = if steps > 0 { steps_done as f64 / steps as f64 } else { 0.0 };
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let status = p.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let progress_pct = (progress * 100.0).round() as u64;
            let inject_preview = if p.get("is_active").and_then(|v| v.as_bool()).unwrap_or(false) {
                format!("当前活跃项目：{} · 进度 {}% · 状态：{}", title, progress_pct, status)
            } else {
                String::new()
            };
            let mut data = p.clone();
            if let Some(obj) = data.as_object_mut() {
                obj.insert("progress".into(), serde_json::json!((progress * 100.0).round() / 100.0));
                obj.insert("steps_done".into(), serde_json::json!(steps_done));
                obj.insert("inject_preview".into(), serde_json::json!(inject_preview));
            }
            Json(serde_json::json!({"status": "ok", "data": data}))
        }
        None => Json(serde_json::json!({"status": "error", "error": "plan not found"})),
    }
}

async fn handle_plan_create(
    Extension(_state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut plans = read_plans(&home);
    let next_id = plans.iter().map(|p| p.get("id").and_then(|v| v.as_u64()).unwrap_or(0)).max().unwrap_or(0) + 1;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let title = payload.get("title").and_then(|v| v.as_str()).unwrap_or("未命名");
    let description = payload.get("description").and_then(|v| v.as_str()).unwrap_or("");
    let tags = payload.get("tags").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let priority = payload.get("priority").and_then(|v| v.as_u64()).unwrap_or(3);
    let plan = serde_json::json!({
        "id": next_id,
        "title": title,
        "description": description,
        "status": "draft",
        "steps": [],
        "tags": tags,
        "created_at": now,
        "updated_at": now,
        "completed_at": null,
        "source": "",
        "priority": priority,
        "is_active": false,
    });
    plans.push(plan);
    write_plans(&home, &plans);
    Json(serde_json::json!({"status": "ok", "id": next_id}))
}

async fn handle_plan_update(
    Extension(_state): Extension<Arc<AppState>>,
    axum::extract::Path(id): Path<String>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut plans = read_plans(&home);
    let plan_id: u64 = id.parse().unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match plans.iter_mut().find(|p| p.get("id").and_then(|v| v.as_u64()).unwrap_or(0) == plan_id) {
        Some(plan) => {
            if let Some(obj) = plan.as_object_mut() {
                if let Some(title) = payload.get("title").and_then(|v| v.as_str()) {
                    obj.insert("title".into(), serde_json::json!(title));
                }
                if let Some(desc) = payload.get("description").and_then(|v| v.as_str()) {
                    obj.insert("description".into(), serde_json::json!(desc));
                }
                if let Some(tags) = payload.get("tags").and_then(|v| v.as_array()) {
                    obj.insert("tags".into(), serde_json::json!(tags));
                }
                if let Some(priority) = payload.get("priority").and_then(|v| v.as_u64()) {
                    obj.insert("priority".into(), serde_json::json!(priority));
                }
                if let Some(steps) = payload.get("steps").and_then(|v| v.as_array()) {
                    obj.insert("steps".into(), serde_json::json!(steps));
                }
                obj.insert("updated_at".into(), serde_json::json!(now));
            }
            write_plans(&home, &plans);
            Json(serde_json::json!({"status": "ok"}))
        }
        None => Json(serde_json::json!({"status": "error", "error": "plan not found"})),
    }
}

async fn handle_plan_delete(
    Extension(_state): Extension<Arc<AppState>>,
    axum::extract::Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut plans = read_plans(&home);
    let plan_id: u64 = id.parse().unwrap_or(0);
    plans.retain(|p| p.get("id").and_then(|v| v.as_u64()).unwrap_or(0) != plan_id);
    write_plans(&home, &plans);
    Json(serde_json::json!({"status": "ok"}))
}

async fn handle_plan_activate(
    Extension(_state): Extension<Arc<AppState>>,
    axum::extract::Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut plans = read_plans(&home);
    let plan_id: u64 = id.parse().unwrap_or(0);
    for p in plans.iter_mut() {
        if let Some(obj) = p.as_object_mut() {
            obj.insert("is_active".into(), serde_json::json!(false));
        }
    }
    if let Some(plan) = plans.iter_mut().find(|p| p.get("id").and_then(|v| v.as_u64()).unwrap_or(0) == plan_id) {
        if let Some(obj) = plan.as_object_mut() {
            obj.insert("is_active".into(), serde_json::json!(true));
        }
    }
    write_plans(&home, &plans);
    Json(serde_json::json!({"status": "ok"}))
}

async fn handle_plan_deactivate(
    Extension(_state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut plans = read_plans(&home);
    for p in plans.iter_mut() {
        if let Some(obj) = p.as_object_mut() {
            obj.insert("is_active".into(), serde_json::json!(false));
        }
    }
    write_plans(&home, &plans);
    Json(serde_json::json!({"status": "ok"}))
}

async fn handle_plan_active(
    Extension(_state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let plans = read_plans(&home);
    let config = read_plan_config(&home);
    let active_plan = plans.iter().find(|p| p.get("is_active").and_then(|v| v.as_bool()).unwrap_or(false));
    let data = match active_plan {
        Some(p) => {
            let steps = p.get("steps").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let steps_done = p.get("steps").and_then(|v| v.as_array()).map(|a| {
                a.iter().filter(|s| s.get("status").and_then(|x| x.as_str()) == Some("completed")).count()
            }).unwrap_or(0);
            let progress = if steps > 0 { steps_done as f64 / steps as f64 } else { 0.0 };
            let title = p.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let status = p.get("status").and_then(|v| v.as_str()).unwrap_or("");
            let progress_pct = (progress * 100.0).round() as u64;
            let injection_preview = {
                let desc = p.get("description").and_then(|v| v.as_str()).unwrap_or("");
                let priority = p.get("priority").and_then(|v| v.as_u64()).unwrap_or(3);
                let tags = p.get("tags").and_then(|v| v.as_array()).map(|a| {
                    a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(", ")
                }).filter(|s| !s.is_empty()).map(|s| format!("标签：{}", s)).unwrap_or_default();
                let current_step = p.get("steps").and_then(|v| v.as_array()).map(|steps| {
                    steps.iter().find(|s| s.get("status").and_then(|x| x.as_str()) == Some("in_progress"))
                        .or_else(|| steps.iter().find(|s| s.get("status").and_then(|x| x.as_str()) == Some("pending")))
                        .and_then(|s| s.get("title").and_then(|t| t.as_str()))
                }).flatten().unwrap_or("");
                let step_hint = if !current_step.is_empty() {
                    format!("下一步：{}", current_step)
                } else if steps > 0 && steps_done == steps {
                    "所有步骤已完结".to_string()
                } else {
                    String::new()
                };
                format!(
                    "[项目] {}：{}\n进度 {}% ({}/{}) · 优先级 P{}\n{}\n{}\n可用：board_view 看全貌 · board_complete_step 标记完成 · board_add_step 加新步骤 · board_activate 切换项目",
                    title, desc, progress_pct, steps_done, steps, priority, step_hint, tags,
                )
            };
            let mut plan_data = p.clone();
            if let Some(obj) = plan_data.as_object_mut() {
                obj.insert("progress".into(), serde_json::json!((progress * 100.0).round() / 100.0));
                obj.insert("steps_done".into(), serde_json::json!(steps_done));
            }
            serde_json::json!({
                "active_plan": plan_data,
                "injection_preview": injection_preview,
                "auto_detect": {
                    "enabled": config.get("auto_detect_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                    "threshold": config.get("auto_detect_threshold").and_then(|v| v.as_f64()).unwrap_or(0.6),
                }
            })
        }
        None => serde_json::json!({
            "active_plan": null,
            "injection_preview": "",
            "auto_detect": {
                "enabled": config.get("auto_detect_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
                "threshold": config.get("auto_detect_threshold").and_then(|v| v.as_f64()).unwrap_or(0.6),
            }
        }),
    };
    Json(serde_json::json!({"status": "ok", "data": data}))
}

async fn handle_plan_settings_get(
    Extension(_state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config = read_plan_config(&home);
    Json(serde_json::json!({
        "status": "ok",
        "data": {
            "auto_detect_enabled": config.get("auto_detect_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
            "auto_detect_threshold": config.get("auto_detect_threshold").and_then(|v| v.as_f64()).unwrap_or(0.6),
            "auto_switch_cooldown_secs": config.get("auto_switch_cooldown_secs").and_then(|v| v.as_u64()).unwrap_or(300),
        }
    }))
}

async fn handle_plan_settings_set(
    Extension(_state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config = serde_json::json!({
        "auto_detect_enabled": payload.get("auto_detect_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        "auto_detect_threshold": payload.get("auto_detect_threshold").and_then(|v| v.as_f64()).unwrap_or(0.6),
        "auto_switch_cooldown_secs": payload.get("auto_switch_cooldown_secs").and_then(|v| v.as_u64()).unwrap_or(300),
    });
    write_plan_config(&home, &config);
    Json(serde_json::json!({"status": "ok"}))
}

// ─── 用户画像 CRUD handler ────────────────────

/// GET /dashboard/user/{uid} — 读取单个用户的完整 TOML 画像
async fn handle_user_get(
    Extension(state): Extension<Arc<AppState>>,
    Path(uid): Path<String>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("users")
        .join(format!("{}.toml", uid));

    if !path.exists() {
        return Json(serde_json::json!({"status": "error", "message": "用户不存在"}));
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => {
            // 解析 TOML 提取别名信息
            let alias_table = match content.parse::<toml::Value>() {
                Ok(val) => {
                    let name = val.get("name").or_else(|| val.get("account_name"))
                        .and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let qq = val.get("qq").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let display_name = val.get("display_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let others: Vec<String> = val.get("alias")
                        .and_then(|a| a.get("others"))
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                        .unwrap_or_default();
                    // 从 alias section 取 name/qq（优先于顶层的旧字段）
                    let alias_name = val.get("alias")
                        .and_then(|a| a.get("name"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or(name);
                    let alias_qq = val.get("alias")
                        .and_then(|a| a.get("qq"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or(qq);
                    let alias_display = val.get("alias")
                        .and_then(|a| a.get("display_name"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                        .unwrap_or(display_name);
                    serde_json::json!({
                        "name": alias_name,
                        "qq": alias_qq,
                        "display_name": alias_display,
                        "others": others,
                    })
                }
                Err(_) => {
                    // TOML 解析失败时，用行级回退从原始字符串提取字段
                    let mut fallback_name = String::new();
                    let mut fallback_qq = String::new();
                    let mut fallback_display = String::new();
                    for line in content.lines() {
                        if let Some(val) = line.strip_prefix("name = \"") {
                            if let Some(end) = val.find('"') {
                                fallback_name = val[..end].to_string();
                            }
                        } else if let Some(val) = line.strip_prefix("qq = \"") {
                            if let Some(end) = val.find('"') {
                                fallback_qq = val[..end].to_string();
                            }
                        } else if let Some(val) = line.strip_prefix("display_name = \"") {
                            if let Some(end) = val.find('"') {
                                fallback_display = val[..end].to_string();
                            }
                        }
                    }
                    serde_json::json!({
                        "name": fallback_name,
                        "qq": fallback_qq,
                        "display_name": fallback_display,
                        "others": serde_json::Value::Array(vec![]),
                    })
                }
            };

            Json(serde_json::json!({
                "status": "ok",
                "uid": uid,
                "toml_content": content,
                "alias_table": alias_table,
            }))
        }
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("读取失败: {}", e)})),
    }
}

/// POST /dashboard/user/create — 新建用户
async fn handle_user_create(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let uid = payload.get("uid").and_then(|v| v.as_str()).unwrap_or("");
    let toml_content = payload.get("toml_content").and_then(|v| v.as_str()).unwrap_or("");

    if uid.is_empty() || toml_content.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "缺少 uid 或 toml_content"}));
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("users")
        .join(format!("{}.toml", uid));

    if path.exists() {
        return Json(serde_json::json!({"status": "error", "message": "用户已存在"}));
    }

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match std::fs::write(&path, toml_content) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "message": "创建成功"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("写入失败: {}", e)})),
    }
}

/// POST /dashboard/user/{uid}/update — 更新用户 TOML 画像
async fn handle_user_update(
    Extension(state): Extension<Arc<AppState>>,
    Path(uid): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let toml_content = payload.get("toml_content").and_then(|v| v.as_str()).unwrap_or("");

    if toml_content.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "缺少 toml_content"}));
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("users")
        .join(format!("{}.toml", uid));

    if !path.exists() {
        return Json(serde_json::json!({"status": "error", "message": "用户不存在"}));
    }

    match std::fs::write(&path, toml_content) {
        Ok(_) => {
            // 验证写入的 TOML 是否可解析
            let validation_warning = match std::fs::read_to_string(&path) {
                Ok(saved) => {
                    if saved.parse::<toml::Value>().is_err() {
                        Some("写入成功但 TOML 格式异常，建议检查")
                    } else {
                        None
                    }
                }
                Err(_) => Some("写入成功但读取验证失败"),
            };
            let mut resp = serde_json::json!({"status": "ok", "message": "更新成功"});
            if let Some(warn) = validation_warning {
                resp["warning"] = serde_json::Value::String(warn.to_string());
            }
            Json(resp)
        }
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("写入失败: {}", e)})),
    }
}

/// POST /dashboard/user/{uid}/delete — 删除用户
async fn handle_user_delete(
    Extension(state): Extension<Arc<AppState>>,
    Path(uid): Path<String>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("users")
        .join(format!("{}.toml", uid));

    if !path.exists() {
        return Json(serde_json::json!({"status": "error", "message": "用户不存在"}));
    }

    match std::fs::remove_file(&path) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "message": "删除成功"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("删除失败: {}", e)})),
    }
}

/// POST /dashboard/user/{uid}/rename — 重命名用户 UID
async fn handle_user_rename(
    Extension(state): Extension<Arc<AppState>>,
    Path(uid): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let new_uid = payload.get("new_uid").and_then(|v| v.as_str()).unwrap_or("");

    if new_uid.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "缺少 new_uid"}));
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let users_dir = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("users");
    let old_path = users_dir.join(format!("{}.toml", uid));
    let new_path = users_dir.join(format!("{}.toml", new_uid));

    if !old_path.exists() {
        return Json(serde_json::json!({"status": "error", "message": "用户不存在"}));
    }

    if new_path.exists() {
        return Json(serde_json::json!({"status": "error", "message": "新 UID 已存在"}));
    }

    match std::fs::rename(&old_path, &new_path) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "message": format!("已重命名为 {}", new_uid)})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("重命名失败: {}", e)})),
    }
}

/// GET /dashboard/user/settings — 读取用户模块配置
async fn handle_user_settings_get(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let profile_dir = std::path::Path::new(&home).join(".tremolite/profiles").join(&state.profile_name);
    let config_path = profile_dir.join("modules").join("user.toml");

    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => {
            // 返回默认配置
            return Json(serde_json::json!({
                "status": "ok",
                "config": {
                    "auto_mode": true,
                    "timed_mode": false,
                    "message_interval": 5
                },
                "config_path": config_path.to_string_lossy().to_string(),
            }));
        }
    };

    match content.parse::<toml::Value>() {
        Ok(val) => {
            let auto_mode = val.get("profile_update")
                .and_then(|p| p.get("auto_mode")).and_then(|v| v.as_bool()).unwrap_or(true);
            let timed_mode = val.get("profile_update")
                .and_then(|p| p.get("timed_mode")).and_then(|v| v.as_bool()).unwrap_or(false);
            let message_interval = val.get("profile_update")
                .and_then(|p| p.get("message_interval")).and_then(|v| v.as_integer()).unwrap_or(5) as u32;
            Json(serde_json::json!({
                "status": "ok",
                "config": {
                    "auto_mode": auto_mode,
                    "timed_mode": timed_mode,
                    "message_interval": message_interval,
                },
                "config_path": config_path.to_string_lossy().to_string(),
            }))
        }
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("配置解析失败: {}", e),
        })),
    }
}

/// POST /dashboard/user/settings — 保存用户模块配置
async fn handle_user_settings_set(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let auto_mode = payload.get("auto_mode").and_then(|v| v.as_bool()).unwrap_or(true);
    let timed_mode = payload.get("timed_mode").and_then(|v| v.as_bool()).unwrap_or(false);
    let message_interval = payload.get("message_interval").and_then(|v| v.as_u64()).unwrap_or(5);

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let profile_dir = std::path::Path::new(&home).join(".tremolite/profiles").join(&state.profile_name);
    let config_path = profile_dir.join("modules").join("user.toml");

    // 创建父目录
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let toml_content = format!(
        "[profile_update]\n\
         auto_mode = {}\n\
         timed_mode = {}\n\
         message_interval = {}\n",
        auto_mode, timed_mode, message_interval
    );

    match std::fs::write(&config_path, &toml_content) {
        Ok(_) => Json(serde_json::json!({
            "status": "ok",
            "message": "配置已保存",
            "config": {
                "auto_mode": auto_mode,
                "timed_mode": timed_mode,
                "message_interval": message_interval,
            },
        })),
        Err(e) => Json(serde_json::json!({
            "status": "error",
            "message": format!("保存失败: {}", e),
        })),
    }
}

async fn handle_config_check() -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_dir = std::path::Path::new(&home).join(".tremolite/profiles/aoi");
    let data_dir = std::path::Path::new(&home).join(".tremolite/data");

    let check_files = vec![
        "emotion.json",
        "emotion_history.json",
        "tone_map.json",
        "cron_tasks.json",
        "SOUL.md",
    ];
    let check_dirs = vec![
        "data/memory/l1_working.json",
        "data/memory/l2_profile.json",
        "data/memory/l3_index.json",
    ];

    let mut in_profile = Vec::new();
    let mut in_data = Vec::new();
    let mut missing = Vec::new();

    for f in &check_files {
        let pf = profile_dir.join(f);
        let df = data_dir.join(f);
        if pf.exists() { in_profile.push(f.to_string()); }
        else if df.exists() { in_data.push(f.to_string()); }
        else { missing.push(f.to_string()); }
    }
    for f in &check_dirs {
        let pf = profile_dir.join(f);
        let df = data_dir.join(f);
        if pf.exists() { in_profile.push(f.to_string()); }
        else if df.exists() { in_data.push(f.to_string()); }
        else { missing.push(f.to_string()); }
    }

    let mut data_leftovers = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&data_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                data_leftovers.push(p.to_string_lossy().to_string());
            }
        }
    }
    for sub in &["memory", "learn"] {
        let subdir = data_dir.join(sub);
        if let Ok(entries) = std::fs::read_dir(&subdir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    data_leftovers.push(p.to_string_lossy().to_string());
                }
            }
        }
    }

    Json(serde_json::json!({
        "status": "ok",
        "profile_path": profile_dir.to_string_lossy().to_string(),
        "data_path": data_dir.to_string_lossy().to_string(),
        "in_profile": in_profile,
        "in_data": in_data,
        "missing": missing,
        "data_leftovers": data_leftovers,
    }))
}

// ─── Handler: 配置数据 ────────────────────────────

async fn handle_dashboard_config() -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home).join(".tremolite").join("config.toml");
    let content = match std::fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(_) => return Json(serde_json::json!({"status": "error", "error": "无法读取配置"})),
    };
    // 按段解析，敏感值脱敏
    let mut sections: Vec<serde_json::Value> = Vec::new();
    let mut current_section = String::new();
    let mut current_lines: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.starts_with('[') && line.ends_with(']') {
            if !current_section.is_empty() {
                sections.push(serde_json::json!({"section": current_section, "lines": current_lines}));
            }
            current_section = line.trim_matches('[').trim_matches(']').to_string();
            current_lines = Vec::new();
        } else if !current_section.is_empty() {
            let masked = if line.contains("api_key") || line.contains("secret") || line.contains("token") || line.contains("avatar") {
                let parts: Vec<&str> = line.splitn(2, '=').collect();
                if parts.len() == 2 {
                    format!("{} = \"********\"", parts[0].trim())
                } else { line.to_string() }
            } else { line.to_string() };
            if !masked.trim().is_empty() {
                current_lines.push(masked);
            }
        }
    }
    if !current_section.is_empty() {
        sections.push(serde_json::json!({"section": current_section, "lines": current_lines}));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_path = std::path::Path::new(&home).join(".tremolite/profiles/aoi/profile.json");
    let profile_exists = profile_path.exists();
    sections.push(serde_json::json!({
        "section": "用户配置 (配置包)",
        "lines": [
            format!("文件路径: {}", profile_path.to_string_lossy()),
            format!("状态: {}", if profile_exists { "已创建 ✓" } else { "未创建" }),
            "提示: 使用下方「用户设定」卡片编辑用户名/AI名/头像".to_string(),
        ]
    }));
    Json(serde_json::json!({"status": "ok", "sections": sections}))
}

// ─── Handler: 日志数据 ────────────────────────────

async fn handle_dashboard_logs() -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    // 尝试从多个位置读日志
    let log_candidates = [
        std::path::Path::new(&home).join(".tremolite").join("logs").join("daemon.log"),
        std::path::Path::new(&home).join("workspace").join("tremolite").join("logs").join("tremolite.log"),
        std::path::PathBuf::from("/tmp/daemon_test.log"),
    ];
    let mut found_logs = Vec::new();
    for path in &log_candidates {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(path) {
                let lines: Vec<&str> = content.lines().rev().take(50).collect::<Vec<_>>();
                let tail: Vec<String> = lines.into_iter().rev().map(|s| s.to_string()).collect();
                found_logs.push(serde_json::json!({
                    "source": path.to_string_lossy(),
                    "lines": tail,
                }));
                if found_logs.len() >= 2 { break; }
            }
        }
    }
    Json(serde_json::json!({"status": "ok", "logs": found_logs}))
}

// ─── Handler: 会话数据 ────────────────────────────

async fn handle_dashboard_sessions(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let uptime = state.uptime_secs();

    // 每次查看 session 列表时触发冷却检查——冷却不依赖消息驱动
    let _ = state.modules.as_ref().map(|mods| {
        mods.with_module_mut("session", |m| {
            m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::SessionModule>())
                .map(|sm| sm.reap_and_save())
        })
    });

    let home = std::env::var("HOME").unwrap_or_default();
    let tremolite_dir = std::path::Path::new(&home).join(".tremolite");

    // 从引擎获取实际 idle_timeout（而不是从文件读，文件可能已过时）
    let engine_idle = state.modules.as_ref()
        .and_then(|mods| mods.with_module("session", |m| {
            m.as_any()
                .and_then(|a| a.downcast_ref::<tremolite_core::SessionModule>())
                .map(|sm| sm.manager.idle_timeout_secs())
        }))
        .flatten()
        .unwrap_or(300);

    // 读 session 配置文件（兜底）
    let config_path = tremolite_dir.join("profiles").join(&state.profile_name).join("modules").join("session.toml");
    let mut idle_timeout: u64 = engine_idle;
    let mut cleanup_timeout: u64 = 2592000;
    let mut max_rings: u64 = 20;
    if let Ok(content) = std::fs::read_to_string(&config_path) {
        if let Ok(toml_val) = content.parse::<toml::Value>() {
            if let Some(cfg) = toml_val.get("session") {
                idle_timeout = cfg.get("idle_timeout").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(engine_idle);
                cleanup_timeout = cfg.get("cleanup_timeout").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(2592000);
                max_rings = cfg.get("max_rings").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(20);
            }
        }
    }

    // 从持久文件读取会话状态
    let state_path = tremolite_dir.join("profiles").join(&state.profile_name).join("sessions").join("session_state.json");
    let mut session_list: Vec<serde_json::Value> = Vec::new();
    let mut active_count: usize = 0;
    let mut shared_count: usize = 0;

    if let Ok(content) = std::fs::read_to_string(&state_path) {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(sessions) = json.get("sessions").and_then(|v| v.as_object()) {
                for (sid, s) in sessions {
                    let closed = s.get("closed").and_then(|v| v.as_bool()).unwrap_or(false);
                    let shared = s.get("shared").and_then(|v| v.as_bool()).unwrap_or(false);
                    let last_active = s.get("last_active").and_then(|v| v.as_u64()).unwrap_or(0);

                    if !closed { active_count += 1; }
                    if shared { shared_count += 1; }

                    let now_ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let ago = if last_active == 0 { 0 } else { now_ts.saturating_sub(last_active) };
                    let last_active_human = if ago < 60 { format!("{}秒前", ago) }
                        else if ago < 3600 { format!("{}分钟前", ago / 60) }
                        else if ago < 86400 { format!("{}小时前", ago / 3600) }
                        else { format!("{}天前", ago / 86400) };

                    session_list.push(serde_json::json!({
                        "id": sid,
                        "shared": shared,
                        "closed": closed,
                        "last_active": last_active,
                        "last_active_human": last_active_human,
                        "status": if closed { "冷却" } else { "活跃" },
                    }));
                }
            }
        }
    }

    // 按最后活跃时间降序排列
    session_list.sort_by(|a, b| {
        let ta = a.get("last_active").and_then(|v| v.as_u64()).unwrap_or(0);
        let tb = b.get("last_active").and_then(|v| v.as_u64()).unwrap_or(0);
        tb.cmp(&ta)
    });

    let idle_human = if idle_timeout < 60 { format!("{}秒", idle_timeout) }
        else if idle_timeout < 3600 { format!("{}分钟", idle_timeout / 60) }
        else { format!("{}小时", idle_timeout / 3600) };
    let cleanup_human = if cleanup_timeout < 86400 { format!("{}小时", cleanup_timeout / 3600) }
        else { format!("{}天", cleanup_timeout / 86400) };

    Json(serde_json::json!({
        "status": "ok",
        "uptime_secs": uptime,
        "uptime_human": {
            "hours": uptime / 3600,
            "minutes": (uptime % 3600) / 60,
            "seconds": uptime % 60,
        },
        "active_session": {
            "platform": "QQ Bot",
            "user": "琳玲",
            "state": "活跃",
        },
        "sessions": session_list,
        "stats": {
            "total": session_list.len(),
            "active": active_count,
            "shared": shared_count,
            "config_source": config_path.to_string_lossy().to_string(),
        },
        "version": SESSION_VERSION,
        "config": {
            "idle_timeout": idle_timeout,
            "idle_timeout_human": idle_human,
            "cleanup_timeout": cleanup_timeout,
            "cleanup_timeout_human": cleanup_human,
            "max_rings": max_rings,
        },
    }))
}

// ─── Handler：Webhook 接收端 ────────────────────

async fn handle_webhook(
    Extension(_state): Extension<Arc<AppState>>,
    axum::extract::Path(name): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<Value> {
    GLOBAL_METRICS.total_requests.fetch_add(1, Ordering::Relaxed);

    // 确定事件来源
    let source = headers.get("X-GitHub-Event")
        .and_then(|v| v.to_str().ok())
        .or_else(|| headers.get("X-Event-Name")
            .and_then(|v| v.to_str().ok()))
        .unwrap_or("custom")
        .to_string();

    let source_str = source.clone();

    // 组装 WebhookEvent
    let event = tremolite_core::WebhookEvent {
        name,
        source,
        headers: headers.iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect(),
        payload,
    };

    // 记录下来
    let msg = format!("webhook received: source={}", source_str);
    tracing::info!("{}", msg);

    // TODO: 后续通过 WebhookModule 处理流水线
    Json(serde_json::json!({
        "status": "ok",
        "message": msg,
        "hook_name": event.name,
        "source": event.source,
    }))
}

// ─── Handler: 记忆搜索 ─────────────────────
async fn handle_memory_search(
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let q = params.get("q").map(|s| s.trim()).unwrap_or("").to_lowercase();
    let home = std::env::var("HOME").unwrap_or_default();
    let (base_dir, _) = memory_dir_path(&home);
    
    let mut results: Vec<serde_json::Value> = Vec::new();
    
    let l2 = read_json_file(&base_dir.join("l2_profile.json"));
    if let Some(obj) = l2.as_object() {
        for (key, val) in obj {
            let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            let tags_str = val.get("tags").and_then(|v| v.as_array()).map(|a| {
                a.iter().filter_map(|t| t.as_str()).collect::<Vec<_>>().join(" ")
            }).unwrap_or_default().to_lowercase();
            if q.is_empty() || content.contains(&q) || tags_str.contains(&q) {
                results.push(serde_json::json!({
                    "layer": "l2", "key": key,
                    "id": val.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                    "content": val.get("content"),
                    "tags": val.get("tags"),
                    "importance": val.get("importance").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                }));
            }
        }
    }
    
    let l3 = read_json_file(&base_dir.join("l3_index.json"));
    if let Some(obj) = l3.as_object() {
        for (key, val) in obj {
            let summary = val.get("summary").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            if q.is_empty() || summary.contains(&q) {
                let id_num: u64 = key.parse().unwrap_or(0);
                let mut ram_content = String::new();
                let ram_file = base_dir.join("ram").join(format!("{}.txt", id_num));
                if let Ok(content) = std::fs::read_to_string(&ram_file) {
                    ram_content = content;
                }
                results.push(serde_json::json!({
                    "layer": "l3", "id": id_num, "id_str": key,
                    "summary": val.get("summary").and_then(|v| v.as_str()).unwrap_or(""),
                    "has_embedding": val.get("embedding").is_some(),
                    "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    "ram_content": ram_content,
                }));
            }
        }
    }
    
    let disk_idx = read_json_file(&base_dir.join("disk_index/index.json"));
    if let Some(obj) = disk_idx.as_object() {
        for (key, val) in obj {
            let keyword = val.get("keyword").and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
            if q.is_empty() || keyword.contains(&q) {
                let id_num: u64 = key.parse().unwrap_or(0);
                let mut store_content = String::new();
                let store_file = base_dir.join("disk_store").join(format!("{}.txt", id_num));
                if let Ok(content) = std::fs::read_to_string(&store_file) {
                    store_content = content;
                }
                results.push(serde_json::json!({
                    "layer": "disk", "id": id_num, "id_str": key,
                    "keyword": val.get("keyword").and_then(|v| v.as_str()).unwrap_or(""),
                    "created_at": val.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0),
                    "store_content": store_content,
                }));
            }
        }
    }
    
    results.sort_by(|a, b| {
        let ca = a.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cb = b.get("created_at").and_then(|v| v.as_f64()).unwrap_or(0.0);
        cb.partial_cmp(&ca).unwrap_or(std::cmp::Ordering::Equal)
    });
    
    Json(serde_json::json!({
        "status": "ok", "query": q, "total": results.len(),
        "results": results,
        "memory_path": base_dir.to_string_lossy().to_string(),
        "in_profile": base_dir.to_string_lossy().contains("/profiles/"),
    }))
}

async fn handle_memory_delete(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let layer = body.get("layer").and_then(|v| v.as_str()).unwrap_or("");
    let id_str = body.get("id_str").and_then(|v| v.as_str()).unwrap_or("");
    let id_num = body.get("id_num").and_then(|v| v.as_u64()).unwrap_or(0);
    let home = std::env::var("HOME").unwrap_or_default();
    let (base_dir, _) = memory_dir_path(&home);
    
    let mut removed = Vec::new();
    
    match layer {
        "l2" => {
            let mut l2 = read_json_file(&base_dir.join("l2_profile.json"));
            if let Some(obj) = l2.as_object_mut() {
                if obj.remove(id_str).is_some() {
                    removed.push(format!("l2:{}", id_str));
                    let _ = write_json_file(&base_dir.join("l2_profile.json"), &l2);
                    for fname in &["l2_embeddings.json", "l2_rough.json"] {
                        let mut f = read_json_file(&base_dir.join(fname));
                        if let Some(o) = f.as_object_mut() { o.remove(id_str); }
                        let _ = write_json_file(&base_dir.join(fname), &f);
                    }
                }
            }
        }
        "l3" => {
            let mut l3 = read_json_file(&base_dir.join("l3_index.json"));
            if let Some(obj) = l3.as_object_mut() {
                if obj.remove(id_str).is_some() {
                    removed.push(format!("l3:{}", id_str));
                    let _ = write_json_file(&base_dir.join("l3_index.json"), &l3);
                    let mut ram = read_json_file(&base_dir.join("ram_fts.json"));
                    if let Some(o) = ram.as_object_mut() { o.remove(id_str); }
                    let _ = write_json_file(&base_dir.join("ram_fts.json"), &ram);
                    let ram_file = base_dir.join("ram").join(format!("{}.txt", id_num));
                    if ram_file.exists() { let _ = std::fs::remove_file(&ram_file); }
                    removed.push(format!("ram:{}", id_num));
                    let vec_file = base_dir.join("ram").join(format!("{}.vec.json", id_num));
                    if vec_file.exists() { let _ = std::fs::remove_file(&vec_file); }
                }
            }
        }
        "disk" => {
            let idx_path = base_dir.join("disk_index/index.json");
            let mut disk_idx = read_json_file(&idx_path);
            if let Some(obj) = disk_idx.as_object_mut() {
                if obj.remove(id_str).is_some() {
                    removed.push(format!("disk_index:{}", id_str));
                    let _ = write_json_file(&idx_path, &disk_idx);
                    let store_file = base_dir.join("disk_store").join(format!("{}.txt", id_num));
                    if store_file.exists() { let _ = std::fs::remove_file(&store_file); }
                    removed.push(format!("disk_store:{}", id_num));
                    let emb_path = base_dir.join("disk_index/embeddings.json");
                    let mut emb = read_json_file(&emb_path);
                    if let Some(o) = emb.as_object_mut() { o.remove(id_str); }
                    let _ = write_json_file(&emb_path, &emb);
                }
            }
        }
        _ => {}
    }
    
    Json(serde_json::json!({"status": "ok", "removed": removed}))
}

async fn handle_memory_update(
    axum::Json(body): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let layer = body.get("layer").and_then(|v| v.as_str()).unwrap_or("");
    let id_str = body.get("id_str").and_then(|v| v.as_str()).unwrap_or("");
    let new_content = body.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let id_num = body.get("id_num").and_then(|v| v.as_u64()).unwrap_or(0);
    let home = std::env::var("HOME").unwrap_or_default();
    let (base_dir, _) = memory_dir_path(&home);
    
    match layer {
        "l2" => {
            let mut l2 = read_json_file(&base_dir.join("l2_profile.json"));
            if let Some(obj) = l2.as_object_mut() {
                if let Some(entry) = obj.get_mut(id_str) {
                    if let Some(m) = entry.as_object_mut() {
                        m.insert("content".into(), serde_json::Value::String(new_content.into()));
                        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                        m.insert("last_updated".into(), serde_json::json!(now));
                    }
                }
            }
            let _ = write_json_file(&base_dir.join("l2_profile.json"), &l2);
        }
        "l3" => {
            let mut l3 = read_json_file(&base_dir.join("l3_index.json"));
            if let Some(obj) = l3.as_object_mut() {
                if let Some(entry) = obj.get_mut(id_str) {
                    if let Some(m) = entry.as_object_mut() {
                        m.insert("summary".into(), serde_json::Value::String(new_content.into()));
                    }
                }
            }
            let _ = write_json_file(&base_dir.join("l3_index.json"), &l3);
            let mut ram = read_json_file(&base_dir.join("ram_fts.json"));
            if let Some(obj) = ram.as_object_mut() {
                if let Some(entry) = obj.get_mut(id_str) {
                    if let Some(m) = entry.as_object_mut() {
                        m.insert("content_preview".into(), serde_json::Value::String(new_content.chars().take(100).collect::<String>()));
                    }
                }
            }
            let _ = write_json_file(&base_dir.join("ram_fts.json"), &ram);
            let ram_file = base_dir.join("ram").join(format!("{}.txt", id_num));
            let _ = std::fs::write(&ram_file, &new_content);
        }
        "disk" => {
            let idx_path = base_dir.join("disk_index/index.json");
            let mut disk_idx = read_json_file(&idx_path);
            if let Some(obj) = disk_idx.as_object_mut() {
                if let Some(entry) = obj.get_mut(id_str) {
                    if let Some(m) = entry.as_object_mut() {
                        m.insert("keyword".into(), serde_json::Value::String(new_content.into()));
                    }
                }
            }
            let _ = write_json_file(&idx_path, &disk_idx);
            let store_file = base_dir.join("disk_store").join(format!("{}.txt", id_num));
            let _ = std::fs::write(&store_file, &new_content);
        }
        _ => {}
    }
    
    Json(serde_json::json!({"status": "ok"}))
}

async fn handle_memory_paths() -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let (base_dir, in_profile) = memory_dir_path(&home);
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&base_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                files.push(path.to_string_lossy().to_string());
            } else if path.is_dir() {
                if let Ok(sub) = std::fs::read_dir(&path) {
                    for se in sub.flatten() {
                        files.push(se.path().to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    Json(serde_json::json!({
        "status": "ok",
        "current_base": base_dir.to_string_lossy().to_string(),
        "in_profile": in_profile,
        "expected_profile_path": std::path::Path::new(&home).join(".tremolite/profiles/aoi/data/memory").to_string_lossy().to_string(),
        "files": files,
    }))
}

// ─── Handler: 保存注意力配置 ──────────────────
async fn handle_attention_update(
    Extension(state): Extension<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let tremolite_dir = std::path::Path::new(&home).join(".tremolite");
    let profile_dir = tremolite_dir.join("profiles").join(&state.profile_name);

    if let Some(weights) = body.get("weights") {
        let attn_json = serde_json::json!({ "weights": weights });
        let attn_path = profile_dir.join("attention.json");
        match std::fs::write(&attn_path, serde_json::to_string_pretty(&attn_json).unwrap()) {
            Ok(_) => tracing::info!("attention: 权重已保存到 {:?}", attn_path),
            Err(e) => return Json(serde_json::json!({ "status": "error", "error": format!("写入attention.json失败: {e}") })),
        }
    }

    if let Some(at) = body.get("auto_tune") {
        let attn_path = profile_dir.join("attention.json");
        let mut current = std::fs::read_to_string(&attn_path).ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .unwrap_or(serde_json::json!({}));
        current["auto_tune"] = at.clone();
        if !current.as_object().map(|o| o.contains_key("weights")).unwrap_or(false) {
            current["weights"] = serde_json::json!({"wide": 1.0, "focus": 2.0, "micro": 3.0});
        }
        match std::fs::write(&attn_path, serde_json::to_string_pretty(&current).unwrap()) {
            Ok(_) => tracing::info!("attention: auto_tune配置已保存"),
            Err(e) => return Json(serde_json::json!({ "status": "error", "error": format!("保存auto_tune失败: {e}") })),
        }
    }

    if let Some(emb) = body.get("embedding") {
        let config_path = profile_dir.join("config.toml");
        let content = std::fs::read_to_string(&config_path).ok().unwrap_or_default();

        let api_base = emb.get("api_base").and_then(|v| v.as_str()).unwrap_or("");
        let api_key = emb.get("api_key").and_then(|v| v.as_str()).unwrap_or("");
        let model = emb.get("model").and_then(|v| v.as_str()).unwrap_or("BAAI/bge-m3");

        let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let mut in_embedding = false;
        let mut embedding_start = None;
        let mut embedding_end = None;
        for (i, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed == "[embedding]" {
                in_embedding = true;
                embedding_start = Some(i);
                continue;
            }
            if in_embedding {
                if trimmed.starts_with('[') {
                    embedding_end = Some(i);
                    break;
                }
            }
        }
        if !in_embedding {
            if !api_base.is_empty() || !api_key.is_empty() {
                lines.push(String::new());
                lines.push("[embedding]".to_string());
                if !api_base.is_empty() { lines.push(format!("api_base = \"{}\"", api_base)); }
                lines.push(format!("api_key = \"{}\"", api_key));
                lines.push(format!("model = \"{}\"", model));
            }
        } else {
            let end = embedding_end.unwrap_or(lines.len());
            if let Some(start) = embedding_start {
                let remove_count = end - start - 1;
                if remove_count > 0 {
                    lines.drain(start+1..end);
                }
                let mut new_lines = Vec::new();
                if !api_base.is_empty() { new_lines.push(format!("api_base = \"{}\"", api_base)); }
                new_lines.push(format!("api_key = \"{}\"", api_key));
                new_lines.push(format!("model = \"{}\"", model));
                for (j, nl) in new_lines.into_iter().enumerate() {
                    lines.insert(start + 1 + j, nl);
                }
            }
        }

        match std::fs::write(&config_path, lines.join("\n") + "\n") {
            Ok(_) => tracing::info!("attention: embedding配置已保存到 {:?}", config_path),
            Err(e) => return Json(serde_json::json!({ "status": "error", "error": format!("写入config.toml失败: {e}") })),
        }
    }

    Json(serde_json::json!({ "status": "ok" }))
}

// ─── 辅助函数 ────────────────────────────────

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m {seconds}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn get_memory_info() -> Value {
    // 从 /proc/self/status 读取 VmRSS
    let rss_kb = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
        });

    if let Some(kb) = rss_kb {
        let mb = kb as f64 / 1024.0;
        serde_json::json!({
            "rss_kb": kb,
            "rss_mb": format!("{:.1}", mb),
        })
    } else {
        serde_json::json!("unavailable")
    }
}

async fn handle_compress_exec(
    Extension(_state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "message": "压缩命令已发送",
    }))
}

async fn handle_compress_settings(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("modules").join("compress.toml");

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut toml_val: toml::Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));

    let compress = toml_val
        .as_table_mut()
        .map(|t| t.entry("compress").or_insert(toml::Value::Table(toml::map::Map::new())))
        .and_then(|v| v.as_table_mut());
    if let Some(t) = compress {
        if let Some(blocks) = payload.get("blocks").and_then(|v| v.as_u64()) {
            t.insert("blocks".into(), toml::Value::Integer(blocks as i64));
        }
        if let Some(threshold) = payload.get("threshold").and_then(|v| v.as_u64()) {
            t.insert("threshold".into(), toml::Value::Integer(threshold as i64));
        }
        if let Some(auto_threshold) = payload.get("auto_threshold").and_then(|v| v.as_bool()) {
            t.insert("auto_threshold".into(), toml::Value::Boolean(auto_threshold));
        }
    }

    match std::fs::write(&config_path, toml_val.to_string()) {
        Ok(_) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("保存失败: {}", e)})),
    }
}

async fn handle_compress_settings_get(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("modules").join("compress.toml");

    let toml_val: toml::Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(toml::Value::Table(toml::map::Map::new()));

    let blocks = toml_val
        .get("compress").and_then(|c| c.get("blocks")).and_then(|v| v.as_integer()).unwrap_or(5);
    let threshold = toml_val
        .get("compress").and_then(|c| c.get("threshold")).and_then(|v| v.as_integer()).unwrap_or(100);
    let auto_threshold = toml_val
        .get("compress").and_then(|c| c.get("auto_threshold")).and_then(|v| v.as_bool()).unwrap_or(false);

    Json(serde_json::json!({
        "status": "ok",
        "data": {
            "blocks": blocks,
            "threshold": threshold,
            "auto_threshold": auto_threshold,
        }
    }))
}

async fn handle_compress_settings_ser(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("modules").join("compress.toml");

    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // 用纯字符串构建 TOML，确保格式正确
    let blocks = payload.get("blocks").and_then(|v| v.as_u64()).unwrap_or(5);
    let threshold = payload.get("threshold").and_then(|v| v.as_u64()).unwrap_or(100);
    let auto_threshold = payload.get("auto_threshold").and_then(|v| v.as_bool()).unwrap_or(false);

    let content = format!(
        "[compress]\nblocks = {}\nthreshold = {}\nauto_threshold = {}\n",
        blocks, threshold, auto_threshold
    );

    match std::fs::write(&config_path, &content) {
        Ok(_) => Json(serde_json::json!({"status": "ok"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("保存失败: {}", e)})),
    }
}

async fn handle_session_config_save(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let config_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("modules").join("session.toml");
    
    let mut idle_timeout: u64 = 300;
    let mut cleanup_timeout: u64 = 2592000;
    let mut max_rings: u64 = 20;
    
    if let Ok(content) = std::fs::read_to_string(&config_path) {
        if let Ok(toml_val) = content.parse::<toml::Value>() {
            if let Some(cfg) = toml_val.get("session") {
                idle_timeout = cfg.get("idle_timeout").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(300);
                cleanup_timeout = cfg.get("cleanup_timeout").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(2592000);
                max_rings = cfg.get("max_rings").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(20);
            }
        }
    }
    
    if let Some(val) = payload.get("idle_timeout_minutes").and_then(|v| v.as_f64()) {
        idle_timeout = (val * 60.0) as u64;
    }
    if let Some(val) = payload.get("cleanup_timeout_days").and_then(|v| v.as_f64()) {
        cleanup_timeout = (val * 86400.0) as u64;
    }
    if let Some(val) = payload.get("max_rings").and_then(|v| v.as_f64()) {
        max_rings = val as u64;
    }
    
    let content = format!(
        "[session]\nidle_timeout = {}\ncleanup_timeout = {}\nmax_rings = {}\n",
        idle_timeout, cleanup_timeout, max_rings
    );
    if let Some(parent) = config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&config_path, content) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "message": "配置已保存"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": format!("保存失败: {}", e)})),
    }
}

async fn handle_session_toggle_share(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if session_id.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "缺少 session_id 参数"}));
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let state_path = std::path::Path::new(&home)
        .join(".tremolite").join("profiles").join(&state.profile_name)
        .join("sessions").join("session_state.json");
    
    let mut data = read_json_file(&state_path);
    let mut new_shared = true;
    if let Some(sessions) = data.get_mut("sessions").and_then(|v| v.as_object_mut()) {
        if let Some(s) = sessions.get_mut(session_id) {
            if let Some(obj) = s.as_object_mut() {
                let current = obj.get("shared").and_then(|v| v.as_bool()).unwrap_or(true);
                new_shared = !current;
                obj.insert("shared".into(), serde_json::json!(new_shared));
            }
        }
    }
    match write_json_file(&state_path, &data) {
        Ok(_) => Json(serde_json::json!({"status": "ok", "shared": new_shared, "message": "已切换"})),
        Err(e) => Json(serde_json::json!({"status": "error", "message": e})),
    }
}

async fn handle_session_activate(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if session_id.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "缺少 session_id 参数"}));
    }

    let result: Option<Result<String, String>> = state.modules.as_ref().and_then(|mods| {
        mods.with_module_mut("session", |m| {
            m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::SessionModule>())
                .map(|sm| sm.activate_session(session_id))
        }).flatten()
    });

    match result {
        Some(Ok(msg)) => Json(serde_json::json!({"status": "ok", "message": msg})),
        Some(Err(e)) => Json(serde_json::json!({"status": "error", "message": e})),
        None => Json(serde_json::json!({"status": "error", "message": "模块不可用"})),
    }
}

async fn handle_session_cooldown(
    Extension(state): Extension<Arc<AppState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let session_id = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
    if session_id.is_empty() {
        return Json(serde_json::json!({"status": "error", "message": "缺少 session_id 参数"}));
    }

    let result: Option<Result<String, String>> = state.modules.as_ref().and_then(|mods| {
        mods.with_module_mut("session", |m| {
            m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::SessionModule>())
                .map(|sm| sm.cooldown_session(session_id))
        }).flatten()
    });

    match result {
        Some(Ok(msg)) => Json(serde_json::json!({"status": "ok", "message": msg})),
        Some(Err(e)) => Json(serde_json::json!({"status": "error", "message": e})),
        None => Json(serde_json::json!({"status": "error", "message": "模块不可用"})),
    }
}

/// 确定记忆文件目录（优先配置包，回退 data 目录）
fn memory_dir_path(home: &str) -> (std::path::PathBuf, bool) {
    let profile_path = std::path::Path::new(home).join(".tremolite/profiles/aoi/data/memory");
    if profile_path.exists() {
        (profile_path, true)
    } else {
        (std::path::Path::new(home).join(".tremolite/data/memory"), false)
    }
}

// ─── Handler: 代谢配置（GET + POST）────────────────

/// 获取当前的代谢配置
async fn handle_metabolism_get(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_path = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("modules/metabolism.toml");
    let toml_str = std::fs::read_to_string(&profile_path).unwrap_or_default();
    let cfg: serde_json::Value = toml::from_str(&toml_str)
        .map(|v: toml::Value| {
            serde_json::to_value(&v).unwrap_or(serde_json::json!({}))
        })
        .unwrap_or(serde_json::json!({}));
    Json(serde_json::json!({
        "status": "ok",
        "data": cfg,
        "path": profile_path.to_string_lossy().to_string(),
    }))
}

/// 保存代谢配置
async fn handle_metabolism_save(
    Extension(state): Extension<Arc<AppState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let home = std::env::var("HOME").unwrap_or_default();
    let profile_path = std::path::Path::new(&home)
        .join(".tremolite/profiles")
        .join(&state.profile_name)
        .join("modules/metabolism.toml");
    if let Some(parent) = profile_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let config = payload.get("config").or(Some(&payload));
    let toml_str = match config {
        Some(v) => {
            let toml_val: toml::Value = serde_json::from_value(v.clone())
                .or_else(|_| serde_json::from_str(&serde_json::to_string(v).unwrap_or_default()))
                .unwrap_or(toml::Value::Table(toml::value::Table::new()));
            toml::to_string_pretty(&toml_val).unwrap_or_default()
        }
        None => String::new(),
    };
    if !toml_str.is_empty() {
        if let Err(e) = std::fs::write(&profile_path, &toml_str) {
            return Json(serde_json::json!({"status": "error", "error": format!("写入失败: {}", e)}));
        }
        tracing::info!("metabolism: config saved to {:?}", profile_path);
    }
    Json(serde_json::json!({"status": "ok"}))
}

/// 读取 JSON 文件为 Value
fn read_json_file(path: &std::path::Path) -> serde_json::Value {
    std::fs::read_to_string(path).ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::json!({}))
}

/// 写 Value 到 JSON 文件
fn write_json_file(path: &std::path::Path, value: &serde_json::Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {}", e))?;
    }
    let s = serde_json::to_string_pretty(value).map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(path, &s).map_err(|e| format!("写入失败: {}", e))?;
    Ok(())
}
