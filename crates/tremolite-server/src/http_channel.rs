use std::sync::Arc;

use axum::{
    Json, Router,
    extract::Path,
    routing::post,
};
use serde::Deserialize;
use tokio::sync::mpsc;

use tremolite_message::{Channel, InboundMessage, OutboundMessage};

// ─── HttpChannel ───────────────────────────────────

/// HTTP 回调通道——接收外部 webhook POST 请求
///
/// 启动后在本机指定地址监听 `POST /webhook/:channel_name`。
/// 请求体 JSON 格式：`{"message": "...", "sender": "..."}`（sender 可选）
pub struct HttpChannel {
    name: String,
    addr: String,
    sender: tokio::sync::Mutex<Option<mpsc::Sender<InboundMessage>>>,
    /// axum 的 graceful shutdown 信号
    shutdown_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl HttpChannel {
    /// 创建新的 HTTP 回调通道
    ///
    /// `name` 是通道标识，`addr` 是监听地址（如 `0.0.0.0:9091`）
    pub fn new(name: &str, addr: &str) -> Self {
        Self {
            name: name.to_string(),
            addr: addr.to_string(),
            sender: tokio::sync::Mutex::new(None),
            shutdown_tx: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Channel for HttpChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn start(&self, sender: mpsc::Sender<InboundMessage>) -> Result<(), String> {
        *self.sender.lock().await = Some(sender);

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // 克隆需要的数据
        let name = self.name.clone();
        let addr = self.addr.clone();

        // 构建路由——用 Arc 包装 sender 供 axum state 使用
        let sender_arc: Arc<mpsc::Sender<InboundMessage>> = Arc::new(
            self.sender.lock().await.clone().unwrap()
        );

        let app = Router::new()
            .route("/webhook/{channel}", post(handle_webhook))
            .with_state(sender_arc);

        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("HttpChannel '{}' failed to bind {addr}: {e}", name))?;

        tracing::info!(
            "channel '{}': HttpChannel listening on http://{addr}/webhook/:channel",
            name
        );

        // 启动服务器
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .ok();
            tracing::info!("channel '{}': HttpChannel stopped", name);
        });

        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        // HttpChannel 是单向接收（入站 webhook），不需要发送回复
        tracing::debug!(
            "channel '{}': outbound to {}: {}",
            self.name,
            msg.target,
            &msg.content[..msg.content.len().min(100)]
        );
        Ok(())
    }

    async fn stop(&self) -> Result<(), String> {
        let mut guard = self.shutdown_tx.lock().await;
        if let Some(tx) = guard.take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

// ─── Webhook Handler ───────────────────────────────

/// 接收 webhook POST 请求
async fn handle_webhook(
    Path(channel): Path<String>,
    state: axum::extract::State<Arc<mpsc::Sender<InboundMessage>>>,
    Json(payload): Json<WebhookPayload>,
) -> Result<Json<serde_json::Value>, axum::http::StatusCode> {
    let msg = InboundMessage::new(
        &payload.message,
        &channel,
        &payload.sender.unwrap_or_else(|| "webhook".into()),
    );

    if state.send(msg).await.is_err() {
        return Err(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(Json(serde_json::json!({ "status": "ok" })))
}

/// Webhook 请求体
#[derive(Debug, Deserialize)]
struct WebhookPayload {
    message: String,
    #[serde(default)]
    sender: Option<String>,
}
