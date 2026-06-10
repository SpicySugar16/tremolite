use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use tremolite_message::{Channel, InboundMessage, OutboundMessage};

// ─── QQ Bot WebSocket 协议类型 ────────────────────

/// WebSocket 帧（QQ Bot Gateway 协议）
#[derive(Debug, Deserialize, Serialize)]
struct WsFrame {
    /// Opcode
    pub op: u64,
    /// 事件数据
    #[serde(default)]
    pub d: serde_json::Value,
    /// 事件类型（仅 op=0 Dispatch）
    #[serde(default)]
    pub t: Option<String>,
    /// 序列号（用于心跳）
    #[serde(default)]
    pub s: Option<u64>,
}

/// Hello 包数据
#[derive(Debug, Deserialize)]
struct HelloData {
    pub heartbeat_interval: u64,
}

/// Identify 包（鉴权）
#[derive(Debug, Serialize)]
struct IdentifyPayload {
    pub token: String,
    pub intents: u64,
    pub shard: Vec<u64>,
}

/// 事件数据（AT_MESSAGE_CREATE / C2C_MESSAGE_CREATE）
#[derive(Debug, Deserialize)]
struct MessageEventData {
    pub id: String,
    pub content: String,
    pub author: MessageAuthor,
    #[serde(default)]
    pub group_open_id: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageAuthor {
    pub id: String,
    #[serde(default)]
    pub username: String,
}

// ─── QQ Bot 常量 ──────────────────────────────────

/// 沙箱环境 API 地址
const SANDBOX_API: &str = "https://api.sgroup.qq.com";
/// 正式环境 API 地址
const PROD_API: &str = "https://api.qq.com";

/// WebSocket Gateway 地址获取路径
const GATEWAY_PATH: &str = "/websocket/";

/// Intents：AT_MESSAGE (1 << 25) | C2C_MESSAGE (1 << 28)
const INTENTS_AT_MESSAGE: u64 = 1 << 25;
const INTENTS_C2C_MESSAGE: u64 = 1 << 28;
const DEFAULT_INTENTS: u64 = INTENTS_AT_MESSAGE | INTENTS_C2C_MESSAGE;

/// Opcodes
const OP_DISPATCH: u64 = 0;
const OP_HEARTBEAT: u64 = 1;
const OP_IDENTIFY: u64 = 2;
const OP_HELLO: u64 = 10;
const OP_HEARTBEAT_ACK: u64 = 11;
const OP_RESUME: u64 = 6;
const OP_RECONNECT: u64 = 7;

// ─── QqBotChannel ─────────────────────────────────

/// QQ 开放平台 Bot 通道
///
/// 通过 QQ Bot 官方 WebSocket Gateway 接收消息，
/// 通过 REST API 发送回复。
pub struct QqBotChannel {
    name: String,
    app_id: String,
    token: String,
    /// True = 正式环境，False = 沙箱
    production: bool,
    shutdown_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl QqBotChannel {
    /// 创建新的 QQ Bot 通道
    ///
    /// `name` 通道标识
    /// `app_id` QQ 开放平台 app_id
    /// `token` QQ 开放平台 token
    /// `production` 是否为正式环境（false = 沙箱）
    pub fn new(name: &str, app_id: &str, token: &str, production: bool) -> Self {
        Self {
            name: name.to_string(),
            app_id: app_id.to_string(),
            token: token.to_string(),
            production,
            shutdown_tx: tokio::sync::Mutex::new(None),
        }
    }

    fn api_base(&self) -> &str {
        if self.production { PROD_API } else { SANDBOX_API }
    }

    fn auth_header(&self) -> String {
        format!("Bot {}.{}", self.app_id, self.token)
    }

    /// 获取 WebSocket 网关地址
    async fn get_gateway_url(&self) -> Result<String, String> {
        let url = format!("{}{}", self.api_base(), GATEWAY_PATH);
        let client = reqwest::Client::new();
        let resp = client
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .map_err(|e| format!("QQ Bot: failed to get gateway URL: {}", e))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("QQ Bot: failed to parse gateway response: {}", e))?;

        data["url"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "QQ Bot: no gateway URL in response".into())
    }
}

#[async_trait::async_trait]
impl Channel for QqBotChannel {
    fn name(&self) -> &str {
        &self.name
    }

    async fn start(&self, sender: mpsc::Sender<InboundMessage>) -> Result<(), String> {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let name = self.name.clone();
        let app_id = self.app_id.clone();
        let token = self.token.clone();
        let api_base = self.api_base().to_string();
        let auth = self.auth_header();

        tokio::spawn(async move {
            // 用 tokio::pin! 固定 shutdown_rx 以便在循环中重复使用
            tokio::pin!(shutdown_rx);

            // 主循环：连接 → 鉴权 → 接收事件（含自动重连）
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx.as_mut() => {
                        tracing::info!("channel '{}': received shutdown signal", name);
                        return;
                    }
                    result = run_qqbot_connection(
                        &name, &app_id, &token, &api_base, &auth, &sender
                    ) => {
                        if let Err(e) = result {
                            tracing::error!("channel '{}': connection error: {}", name, e);
                        }
                        // 断线后等待重连
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                }
            }
        });

        Ok(())
    }

    async fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        // target 格式："group:group_openid" 或 "private:user_openid"
        let parts: Vec<&str> = msg.target.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(format!(
                "QqBotChannel: invalid target '{}', expected 'group:openid' or 'private:openid'",
                msg.target
            ));
        }

        let target_type = parts[0];
        let open_id = parts[1];

        let url = match target_type {
            "group" => format!("{}/v2/groups/{}/messages", self.api_base(), open_id),
            "private" => format!("{}/v2/users/{}/messages", self.api_base(), open_id),
            _ => return Err(format!(
                "QqBotChannel: unknown target type '{}'", target_type
            )),
        };

        let body = serde_json::json!({
            "content": msg.content,
            "msg_type": 0,
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("QqBotChannel: HTTP error: {}", e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            tracing::warn!(
                "channel '{}': send message returned {}: {}",
                self.name, status, text
            );
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

// ─── WebSocket 连接管理 ────────────────────────────

async fn run_qqbot_connection(
    name: &str,
    _app_id: &str,
    token: &str,
    _api_base: &str,
    auth: &str,
    sender: &mpsc::Sender<InboundMessage>,
) -> Result<(), String> {
    // 1. 获取 WebSocket 网关地址
    let ws_url = get_gateway_url_raw(auth).await?;

    tracing::info!("channel '{}': connecting to QQ Bot Gateway at {}", name, ws_url);

    // 2. 连接 WebSocket
    let (ws_stream, _) = connect_async(ws_url.as_str())
        .await
        .map_err(|e| format!("QQ Bot: WebSocket connect failed: {}", e))?;

    let (mut write, mut read) = ws_stream.split();

    // 3. 接收 Hello 包
    let hello_raw = read.next().await
        .ok_or_else(|| "QQ Bot: connection closed before hello".to_string())?
        .map_err(|e| format!("QQ Bot: hello receive error: {}", e))?;

    let hello_text = match &hello_raw {
        Message::Text(t) => t.clone(),
        _ => return Err("QQ Bot: expected text hello frame".into()),
    };

    let hello_frame: WsFrame = serde_json::from_str(&hello_text)
        .map_err(|e| format!("QQ Bot: parse hello failed: {}", e))?;

    if hello_frame.op != OP_HELLO {
        return Err(format!("QQ Bot: expected hello (op 10), got op {}", hello_frame.op));
    }

    let hello_data: HelloData = serde_json::from_value(hello_frame.d)
        .map_err(|e| format!("QQ Bot: parse hello data failed: {}", e))?;

    let heartbeat_interval_ms = hello_data.heartbeat_interval;
    tracing::info!(
        "channel '{}': received hello, heartbeat interval {}ms",
        name, heartbeat_interval_ms
    );

    // 4. 发送 Identify 鉴权
    let identify = WsFrame {
        op: OP_IDENTIFY,
        d: serde_json::to_value(IdentifyPayload {
            token: auth.to_string(),
            intents: DEFAULT_INTENTS,
            shard: vec![0, 1],
        }).unwrap_or_default(),
        t: None,
        s: None,
    };

    let identify_raw = serde_json::to_string(&identify)
        .map_err(|e| format!("QQ Bot: serialize identify failed: {}", e))?;

    write.send(Message::Text(identify_raw.into()))
        .await
        .map_err(|e| format!("QQ Bot: send identify failed: {}", e))?;

    tracing::info!("channel '{}': identify sent, waiting for ready", name);

    // 5. 心跳管理——内联定时器
    let mut last_seq: Option<u64> = None;
    let mut hb_interval = tokio::time::interval(Duration::from_millis(heartbeat_interval_ms));
    hb_interval.tick().await; // 第一次立即 tick

    // 6. 事件接收循环
    loop {
        tokio::select! {
            _ = hb_interval.tick() => {
                // 发送心跳
                let hb_frame = WsFrame {
                    op: OP_HEARTBEAT,
                    d: serde_json::json!(last_seq),
                    t: None,
                    s: None,
                };
                if let Ok(hb_data) = serde_json::to_string(&hb_frame) {
                    if let Err(e) = write.send(Message::Text(hb_data.into())).await {
                        tracing::warn!("channel '{}': heartbeat send error: {}", name, e);
                        return Err(format!("heartbeat send error: {}", e));
                    }
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let frame: WsFrame = match serde_json::from_str(&text) {
                            Ok(f) => f,
                            Err(e) => {
                                tracing::warn!("channel '{}': parse frame failed: {}", name, e);
                                continue;
                            }
                        };

                        match frame.op {
                            OP_DISPATCH => {
                                // 更新序列号
                                if let Some(s) = frame.s {
                                    last_seq = Some(s);
                                }

                                let event_type = frame.t.as_deref().unwrap_or("UNKNOWN");
                                match event_type {
                                    "READY" => {
                                        tracing::info!("channel '{}': ready received, connected", name);
                                    }
                                    "AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" => {
                                        if let Some(data) = parse_message_event(&frame.d) {
                                            let is_group = event_type == "AT_MESSAGE_CREATE";
                                            let channel_detail = if is_group {
                                                let gid = data.group_open_id.as_deref().unwrap_or("unknown");
                                                format!("qqbot.group.{}", gid)
                                            } else {
                                                format!("qqbot.private.{}", data.author.id)
                                            };

                                            // 构建 target 用于回复
                                            let target = if is_group {
                                                if let Some(ref gid) = data.group_open_id {
                                                    format!("group:{}", gid)
                                                } else {
                                                    format!("private:{}", data.author.id)
                                                }
                                            } else {
                                                format!("private:{}", data.author.id)
                                            };

                                            let mut msg = InboundMessage::new(
                                                &data.content,
                                                &channel_detail,
                                                &data.author.id,
                                            );
                                            msg.metadata.insert("qqbot_msg_id".into(), data.id);
                                            msg.metadata.insert("qqbot_target".into(), target);
                                            msg.metadata.insert("nickname".into(), data.author.username);
                                            msg.metadata.insert("event_type".into(), event_type.into());

                                            let _ = sender.send(msg).await;
                                        }
                                    }
                                    "RESUMED" => {
                                        tracing::info!("channel '{}': session resumed", name);
                                    }
                                    _ => {
                                        tracing::debug!("channel '{}': unhandled event: {}", name, event_type);
                                    }
                                }
                            }
                            OP_HEARTBEAT_ACK => {
                                // 心跳确认——不做特殊处理，inline interval 会继续
                            }
                            OP_RECONNECT => {
                                tracing::warn!("channel '{}': server requested reconnect", name);
                                return Err("server requested reconnect".into());
                            }
                            OP_HELLO => {
                                // 重连后的 hello，忽略（已在外部处理）
                            }
                            _ => {
                                tracing::debug!("channel '{}': unhandled op {}", name, frame.op);
                            }
                        }
                    }
                    Some(Ok(Message::Close(reason))) => {
                        tracing::warn!("channel '{}': WS closed: {:?}", name, reason);
                        return Err(format!("WS closed: {:?}", reason));
                    }
                    Some(Err(e)) => {
                        tracing::error!("channel '{}': WS error: {}", name, e);
                        return Err(format!("WS error: {}", e));
                    }
                    None => {
                        tracing::warn!("channel '{}': WS stream ended", name);
                        return Err("WS stream ended".into());
                    }
                    _ => {}
                }
            }
        }
    }
}

/// 获取 QQ Bot Gateway WebSocket 地址（HTTP API）
async fn get_gateway_url_raw(auth: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api.sgroup.qq.com/websocket/")
        .header("Authorization", auth)
        .send()
        .await
        .map_err(|e| format!("QQ Bot: failed to get gateway URL: {}", e))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("QQ Bot: failed to parse gateway response: {}", e))?;

    data["url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "QQ Bot: no gateway URL in response".into())
}

/// 解析消息事件
fn parse_message_event(d: &serde_json::Value) -> Option<MessageEventData> {
    let id = d["id"].as_str()?.to_string();
    let content = d["content"].as_str()?.to_string();

    let author = MessageAuthor {
        id: d["author"]["id"].as_str().unwrap_or("").to_string(),
        username: d["author"]["username"].as_str().unwrap_or("").to_string(),
    };

    Some(MessageEventData {
        id,
        content,
        author,
        group_open_id: d["group_open_id"].as_str().map(String::from),
        guild_id: d["guild_id"].as_str().map(String::from),
        channel_id: d["channel_id"].as_str().map(String::from),
    })
}
