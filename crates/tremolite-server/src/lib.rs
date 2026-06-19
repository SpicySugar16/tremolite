use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc, atomic::{AtomicU64, Ordering}};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::{
    Router,
    extract::{Extension, WebSocketUpgrade, ws::{Message, WebSocket}},
    response::{Html, Json, IntoResponse},
    routing::{get, post},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tower_http::cors::CorsLayer;

use tremolite_core::scheduler::SessionTask;
use tremolite_dashboard::DASHBOARD_HTML;
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
) -> Result<(), String> {
    run_server_inner(inbound_tx, pending_results, addr).await
}

async fn run_server_inner(
    inbound_tx: mpsc::Sender<SessionTask>,
    pending_results: Arc<Mutex<HashMap<String, mpsc::Sender<String>>>>,
    addr: &str,
) -> Result<(), String> {
    let state = Arc::new(AppState {
        inbound_tx,
        pending_results,
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default(),
        total_requests: AtomicU64::new(0),
        active_connections: AtomicU64::new(0),
    });

    let mut router = Router::new()
        .route("/health", get(handle_health))
        .route("/metrics", get(handle_metrics))
        .route("/chat", post(handle_chat))
        .route("/webhooks/{name}", post(handle_webhook))
        .route("/ws", get(handle_ws))
        .route("/dashboard", get(handle_dashboard))
        .route("/dashboard/status", get(handle_dashboard_status))
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
    println!("  POST /chat          —  send message to agent");
    println!("  WS   /ws            —  WebSocket chat");
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
) -> ChannelRegistry {
    let mut registry = ChannelRegistry::new();

    for (key, config) in channels_config {
        match config {
            tremolite_config::ChannelConfig::Http { listen, name } => {
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
            tremolite_config::ChannelConfig::NapCat { ws_url, name } => {
                let channel_name = name.clone().unwrap_or_else(|| key.clone());
                let channel = tremolite_channels::NapCatChannel::new(&channel_name, ws_url);

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
            tremolite_config::ChannelConfig::QqBot { app_id, client_secret, token: _token, production, name } => {
                let channel_name = name.clone().unwrap_or_else(|| key.clone());
                let channel = tremolite_channels::QqBotChannel::new(
                    &channel_name, app_id, client_secret, *production,
                );

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
        "version": "0.2.0",
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

async fn handle_dashboard() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn handle_dashboard_status(
    Extension(state): Extension<Arc<AppState>>,
) -> Json<Value> {
    let uptime = state.uptime_secs();

    Json(serde_json::json!({
        "status": "ok",
        "uptime_secs": uptime,
        "uptime_human": format_uptime(uptime),
        "metrics": {
            "total_requests": GLOBAL_METRICS.total_requests.load(Ordering::Relaxed),
            "active_connections": state.active_connections.load(Ordering::Relaxed),
        },
    }))
}

// ─── Handler：Webhook 接收端 ────────────────────

async fn handle_webhook(
    Extension(state): Extension<Arc<AppState>>,
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
