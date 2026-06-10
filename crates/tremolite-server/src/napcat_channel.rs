use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use tremolite_message::{Channel, InboundMessage, OutboundMessage};

// ─── NapCat OneBot 事件类型 ────────────────────────

/// OneBot 11 标准事件（仅解析需要的字段）
#[derive(Debug, Deserialize)]
struct OneBotEvent {
    /// 事件类型：message, notice, request, meta_event
    post_type: String,
    /// 消息类型：group, private（仅 message 事件有）
    #[serde(default)]
    message_type: String,
    /// 子类型
    #[serde(default)]
    sub_type: String,
    /// 群号
    #[serde(default)]
    group_id: Option<i64>,
    /// 用户 QQ
    #[serde(default)]
    user_id: Option<i64>,
    /// 消息内容（可能为字符串或数组）
    #[serde(default)]
    raw_message: String,
    /// 发送者信息
    #[serde(default)]
    sender: Option<SenderInfo>,
}

#[derive(Debug, Deserialize, Default)]
struct SenderInfo {
    #[serde(default)]
    nickname: String,
    #[serde(default)]
    user_id: Option<i64>,
}

// ─── NapCatChannel ─────────────────────────────────

/// NapCat 消息通道——通过 WebSocket 连接 NapCat QQ 机器人框架
///
/// 生命周期：
/// 1. `start()` 连接 NapCat WebSocket，接收 OneBot 事件
/// 2. 提取消息事件，转发给引擎
/// 3. `send()` 通过 NapCat HTTP API 发送回复
pub struct NapCatChannel {
    name: String,
    ws_url: String,
    http_base: String,
    /// 持有 sender 引用（用于 WS 断线重连时保留 sender）
    sender_mutex: tokio::sync::Mutex<Option<mpsc::Sender<InboundMessage>>>,
    /// 关闭信号
    shutdown_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl NapCatChannel {
    /// 创建新的 NapCat 通道
    ///
    /// `name` 通道标识
    /// `ws_url` WebSocket 地址，如 `ws://localhost:3001/ws`
    pub fn new(name: &str, ws_url: &str) -> Self {
        // 从 ws_url 推导 http_base
        let http_base = ws_url
            .replace("wss://", "https://")
            .replace("ws://", "http://")
            .trim_end_matches("/ws")
            .trim_end_matches('/')
            .to_string();

        Self {
            name: name.to_string(),
            ws_url: ws_url.to_string(),
            http_base,
            sender_mutex: tokio::sync::Mutex::new(None),
            shutdown_tx: tokio::sync::Mutex::new(None),
        }
    }
}

#[async_trait::async_trait]
impl Channel for NapCatChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn start(&self, sender: mpsc::Sender<InboundMessage>) -> Result<(), String> {
        *self.sender_mutex.lock().await = Some(sender.clone());

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let name = self.name.clone();
        let ws_url = self.ws_url.clone();
        let http_base = self.http_base.clone();

        // 后台任务：连接 NapCat WS 并持续接收事件
        tokio::spawn(async move {
            loop {
                // 检查是否收到关闭信号
                if shutdown_rx.try_recv().is_ok() {
                    tracing::info!("channel '{}': received shutdown signal", name);
                    break;
                }

                // 连接 WebSocket（用 &str 直接连，不需要 Url 解析）
                match connect_async(ws_url.as_str()).await {
                    Ok((ws_stream, _response)) => {
                        tracing::info!("channel '{}': connected to NapCat WS", name);

                        let (mut write, mut read) = ws_stream.split();

                        // 读取消息循环
                        loop {
                            tokio::select! {
                                msg = read.next() => {
                                    match msg {
                                        Some(Ok(Message::Text(text))) => {
                                            if let Err(e) = handle_event(&sender, &text, &name, &http_base).await {
                                                tracing::warn!("channel '{}': event handling error: {}", name, e);
                                            }
                                        }
                                        Some(Ok(Message::Ping(data))) => {
                                            let _ = write.send(Message::Pong(data)).await;
                                        }
                                        Some(Ok(Message::Close(_))) => {
                                            tracing::warn!("channel '{}': WS connection closed by server", name);
                                            break;
                                        }
                                        Some(Err(e)) => {
                                            tracing::error!("channel '{}': WS error: {}", name, e);
                                            break;
                                        }
                                        None => {
                                            tracing::warn!("channel '{}': WS stream ended", name);
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                                _ = &mut shutdown_rx => {
                                    tracing::info!("channel '{}': shutdown via signal", name);
                                    return;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            "channel '{}': failed to connect to NapCat at '{}': {}",
                            name, ws_url, e
                        );
                        // 重连等待
                        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        // target 格式："group:123456" 或 "private:654321"
        let parts: Vec<&str> = msg.target.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!(
                "NapCatChannel: invalid target '{}', expected 'group:id' or 'private:id'",
                msg.target
            ));
        }

        let target_type = parts[0];
        let target_id: i64 = parts[1]
            .parse()
            .map_err(|e| format!("NapCatChannel: invalid target id '{}': {}", parts[1], e))?;

        let client = reqwest::Client::new();

        match target_type {
            "group" => {
                let body = serde_json::json!({
                    "group_id": target_id,
                    "message": msg.content
                });
                let url = format!("{}/send_group_msg", self.http_base);
                let resp = client
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| format!("NapCatChannel: HTTP error: {}", e))?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        "channel '{}': send_group_msg returned {}: {}",
                        self.name, status, text
                    );
                }
            }
            "private" => {
                let body = serde_json::json!({
                    "user_id": target_id,
                    "message": msg.content
                });
                let url = format!("{}/send_private_msg", self.http_base);
                let resp = client
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| format!("NapCatChannel: HTTP error: {}", e))?;

                if !resp.status().is_success() {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    tracing::warn!(
                        "channel '{}': send_private_msg returned {}: {}",
                        self.name, status, text
                    );
                }
            }
            _ => {
                return Err(format!(
                    "NapCatChannel: unknown target type '{}', expected 'group' or 'private'",
                    target_type
                ));
            }
        }

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

// ─── 事件处理 ──────────────────────────────────────

/// 处理一条 NapCat WS 推送的事件
async fn handle_event(
    sender: &mpsc::Sender<InboundMessage>,
    text: &str,
    _channel_name: &str,
    _http_base: &str,
) -> Result<(), String> {
    let event: OneBotEvent = serde_json::from_str(text)
        .map_err(|e| format!("failed to parse OneBot event: {}", e))?;

    // 只处理 message 事件
    if event.post_type != "message" {
        return Ok(());
    }

    // 提取文本消息（CQ 码 / 纯文本）
    let raw = event.raw_message.trim().to_string();
    if raw.is_empty() {
        return Ok(());
    }

    // 构造 channel + target 标识
    let (channel_detail, sender_id) = match event.message_type.as_str() {
        "group" => {
            let gid = event.group_id.unwrap_or(0);
            let uid = event.user_id.unwrap_or(0);
            (format!("napcat.group.{}", gid), uid.to_string())
        }
        "private" => {
            let uid = event.user_id.unwrap_or(0);
            (format!("napcat.private.{}", uid), uid.to_string())
        }
        _ => return Ok(()),
    };

    let nickname = event
        .sender
        .as_ref()
        .map(|s| s.nickname.clone())
        .unwrap_or_default();

    let mut msg = InboundMessage::new(&raw, &channel_detail, &sender_id);
    msg.metadata.insert("nickname".into(), nickname);
    msg.metadata.insert("message_type".into(), event.message_type.clone());

    let _ = sender.send(msg).await;

    Ok(())
}
