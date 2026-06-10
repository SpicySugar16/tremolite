use std::sync::Arc;
use std::thread;
use std::time::Duration;

use axum::{
    Json, Router,
    extract::Path,
    routing::post,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub use tremolite_message::{Channel, ChannelRegistry, InboundMessage, OutboundMessage};

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

/// Intents（QQ Bot API v2 最新）
/// 1<<25 = AT_MESSAGE_CREATE (群聊@消息)
/// 1<<30 = GROUP_AT_MESSAGE_CREATE (新版群聊@消息, 也覆盖C2C私聊)
/// 1<<12 = DIRECT_MESSAGE_CREATE (频道私信)
/// 1<<26 = INTERACTION_CREATE (互动事件/按钮)
const INTENTS_AT_MESSAGE: u64 = 1 << 25;
const INTENTS_GROUP_AT_MESSAGE: u64 = 1 << 30;
const INTENTS_DIRECT_MESSAGE: u64 = 1 << 12;
const INTENTS_INTERACTION: u64 = 1 << 26;
const DEFAULT_INTENTS: u64 = INTENTS_AT_MESSAGE | INTENTS_GROUP_AT_MESSAGE
    | INTENTS_DIRECT_MESSAGE | INTENTS_INTERACTION;

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
    client_secret: String,
    /// True = 正式环境，False = 沙箱
    production: bool,
    /// 从 OAuth2 获取的 access_token（运行时动态刷新）
    access_token: tokio::sync::Mutex<Option<(String, std::time::Instant)>>,
    shutdown_tx: tokio::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

impl QqBotChannel {
    /// 创建新的 QQ Bot 通道
    ///
    /// `name` 通道标识
    /// `app_id` QQ 开放平台 app_id
    /// `client_secret` QQ 开放平台 client_secret（用于 OAuth2 获取 access_token）
    /// `production` 是否为正式环境（false = 沙箱）
    pub fn new(name: &str, app_id: &str, client_secret: &str, production: bool) -> Self {
        Self {
            name: name.to_string(),
            app_id: app_id.to_string(),
            client_secret: client_secret.to_string(),
            production,
            access_token: tokio::sync::Mutex::new(None),
            shutdown_tx: tokio::sync::Mutex::new(None),
        }
    }

    fn api_base(&self) -> &str {
        if self.production { PROD_API } else { SANDBOX_API }
    }

    fn token_url() -> &'static str {
        "https://bots.qq.com/app/getAppAccessToken"
    }

    /// 获取 OAuth2 access_token（带缓存和自动刷新）
    async fn get_access_token(&self) -> Result<String, String> {
        let mut guard = self.access_token.lock().await;
        // 检查缓存是否有效（过期前留 60 秒余量）
        if let Some((token, expires_at)) = &*guard {
            if std::time::Instant::now() < *expires_at {
                return Ok(token.clone());
            }
        }

        // 请求新 token
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let resp = client
            .post(Self::token_url())
            .json(&serde_json::json!({
                "appId": self.app_id,
                "clientSecret": self.client_secret,
            }))
            .send()
            .await
            .map_err(|e| format!("QQ Bot: OAuth2 request failed: {e}"))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("QQ Bot: OAuth2 parse failed: {e}"))?;

        let token = data["access_token"]
            .as_str()
            .ok_or_else(|| format!("QQ Bot: OAuth2 response missing access_token: {data}"))?
            .to_string();

        let expires_in: u64 = data["expires_in"].as_u64().unwrap_or(7200);
        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(expires_in - 60);

        *guard = Some((token.clone(), expires_at));
        Ok(token)
    }

    /// 构建 QQBot 认证头值
    async fn auth_bearer(&self) -> Result<String, String> {
        let token = self.get_access_token().await?;
        Ok(format!("QQBot {token}"))
    }

    /// 获取 WebSocket 网关地址（通过 /gateway 发现）
    async fn get_gateway_url(&self) -> Result<String, String> {
        let bearer = self.auth_bearer().await?;
        let url = format!("{}/gateway", self.api_base());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let resp = client
            .get(&url)
            .header("Authorization", &bearer)
            .send()
            .await
            .map_err(|e| format!("QQ Bot: failed to get gateway URL: {e}"))?;

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("QQ Bot: failed to parse gateway response: {e}"))?;

        data["url"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("QQ Bot: no gateway URL in response: {data}"))
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
        let client_secret = self.client_secret.clone();
        let api_base = self.api_base().to_string();

        tokio::spawn(async move {
            // 用 tokio::pin! 固定 shutdown_rx 以便在循环中重复使用
            tokio::pin!(shutdown_rx);

            // OAuth2 token 和 gateway URL 的重复使用缓存
            let mut cached_token: Option<(String, std::time::Instant)> = None;

            // 主循环：连接 → 鉴权 → 接收事件（含自动重连）
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx.as_mut() => {
                        tracing::info!("channel '{}': received shutdown signal", name);
                        return;
                    }
                    result = run_qqbot_connection(
                        &name, &app_id, &client_secret, &api_base, &sender, &mut cached_token
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

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let bearer = self.get_access_token().await
            .map(|t| format!("QQBot {t}"))?;
        let resp = client
            .post(&url)
            .header("Authorization", &bearer)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("QqBotChannel: HTTP error: {e}"))?;

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
    app_id: &str,
    client_secret: &str,
    api_base: &str,
    sender: &mpsc::Sender<InboundMessage>,
    cached_token: &mut Option<(String, std::time::Instant)>,
) -> Result<(), String> {
    // 1. 获取/刷新 OAuth2 access_token
    let access_token = get_or_refresh_token(cached_token, app_id, client_secret).await?;
    let bearer = format!("QQBot {access_token}");

    // 2. 获取 WebSocket 网关地址（通过 /gateway 发现）
    let gateway_url = get_gateway_url(api_base, &bearer).await?;

    tracing::info!("channel '{name}': connecting to QQ Bot Gateway at {gateway_url}");
    // 2. 连接 WebSocket
    let (ws_stream, _) = connect_async(gateway_url.as_str())
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
            token: format!("QQBot {access_token}"),
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
                                    "AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE" | "GROUP_AT_MESSAGE_CREATE" => {
                                        if let Some(data) = parse_message_event(&frame.d) {
                                            let is_group = event_type != "C2C_MESSAGE_CREATE";
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
                                                name,
                                                &target,
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

// ─── OAuth2 & Gateway 辅助函数 ──────────────────────

/// 获取/刷新 QQ Bot OAuth2 access_token（带内存缓存）
async fn get_or_refresh_token(
    cached: &mut Option<(String, std::time::Instant)>,
    app_id: &str,
    client_secret: &str,
) -> Result<String, String> {
    // 如果缓存有效则直接返回
    if let Some((token, expires_at)) = cached {
        if std::time::Instant::now() < *expires_at {
            return Ok(token.clone());
        }
    }

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post("https://bots.qq.com/app/getAppAccessToken")
        .json(&serde_json::json!({
            "appId": app_id,
            "clientSecret": client_secret,
        }))
        .send()
        .await
        .map_err(|e| format!("QQ Bot: OAuth2 request failed: {e}"))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("QQ Bot: OAuth2 parse failed: {e}"))?;

    let token = data["access_token"]
        .as_str()
        .ok_or_else(|| format!("QQ Bot: OAuth2 response missing access_token: {data}"))?
        .to_string();

    let expires_in: u64 = data["expires_in"].as_u64().unwrap_or(7200);
    let expires_at = std::time::Instant::now()
        + std::time::Duration::from_secs(expires_in.saturating_sub(60));

    *cached = Some((token.clone(), expires_at));
    Ok(token)
}

/// 通过 /gateway 发现 WebSocket 网关地址
async fn get_gateway_url(api_base: &str, bearer: &str) -> Result<String, String> {
    let url = format!("{api_base}/gateway");
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(&url)
        .header("Authorization", bearer)
        .send()
        .await
        .map_err(|e| format!("QQ Bot: failed to get gateway URL: {e}"))?;

    let data: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("QQ Bot: failed to parse gateway response: {e}"))?;

    data["url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("QQ Bot: no gateway URL in response: {data}"))
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

        let client = reqwest::Client::builder().no_proxy().build().unwrap();

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

// ─── HttpChannel ───────────────────────────────────

/// HTTP 回调通道——接收外部 webhook POST 请求
///
/// 启动后在本机指定地址监听 `POST /webhook/:channel_name`。
/// 请求体 JSON 格式：`{\"message\": \"...\", \"sender\": \"...\"}`（sender 可选）
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

// ─── ChannelsModule ───────────────────────────────────

use std::any::Any;
use tokio::sync::Mutex as TokioMutex;
use tremolite_core::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError,
};
use tremolite_llm::{ToolDefinition, ToolFunction};

/// 消息通道模块——将 ChannelRegistry 托管为引擎的一个模块
///
/// - 管理所有外部消息通道（QQ Bot、NapCat、HTTP）
/// - 启动时自动注册并启动通道
/// - 桥接到 SessionScheduler 后，入站消息自动转发给调度器
/// - 提供 `list_channels()`, `send_message()` 等工具
pub struct ChannelsModule {
    /// 注册表（桥接后 registry 中的 channels 被移入 bridge_registry）
    registry: ChannelRegistry,
    /// 桥接后共享的注册表（Arc + tokio Mutex 供桥线程与工具共用）
    bridge_registry: Option<Arc<TokioMutex<ChannelRegistry>>>,
    /// 工具发消息到桥线程的通道
    outbound_tool_tx: Option<tokio::sync::mpsc::Sender<OutboundMessage>>,
    /// 是否已桥接
    bridged: bool,
    /// 模块初始化时拿到的 engine handle
    engine_handle: Option<tremolite_core::module::EngineHandle>,
    /// 通道名列表（用于 display_status）
    channel_names: Vec<String>,
}

impl ChannelsModule {
    /// 从已有的 ChannelRegistry 创建 ChannelsModule
    pub fn from_registry(registry: ChannelRegistry) -> Self {
        let channel_names = registry.list_channels();
        Self {
            registry,
            bridge_registry: None,
            outbound_tool_tx: None,
            bridged: false,
            engine_handle: None,
            channel_names,
        }
    }

    pub fn new() -> Self {
        Self {
            registry: ChannelRegistry::new(),
            bridge_registry: None,
            outbound_tool_tx: None,
            bridged: false,
            engine_handle: None,
            channel_names: Vec::new(),
        }
    }

    /// 注册并启动一个通道（可在 init 后调用，也可以在外部先注册好再传入）
    pub async fn register_channel(&mut self, channel: Box<dyn Channel>) -> Result<(), String> {
        let name = channel.name().to_string();
        self.registry.register(channel).await?;
        self.channel_names.push(name);
        Ok(())
    }

    /// 获取入站消息接收器（引擎主循环消费）
    pub fn take_rx(&mut self) -> Option<tokio::sync::mpsc::Receiver<InboundMessage>> {
        self.registry.take_rx()
    }

    /// 向指定通道发送消息（桥接后仍可用，底层走 bridge_registry）
    pub async fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        if let Some(ref reg) = self.bridge_registry {
            reg.lock().await.send(msg).await
        } else {
            self.registry.send(msg).await
        }
    }

    /// 列出所有已注册的通道名
    pub fn list_channels(&self) -> Vec<String> {
        self.channel_names.clone()
    }

    /// 将通道模块桥接到 SessionScheduler
    ///
    /// 接收 tokio 世界（通道 WebSocket/HTTP）的消息，
    /// 转换成 SessionTask 发送到调度器的 std::sync 入站通道。
    /// 同时监听调度器的出站消息，转发回对应的通道。
    ///
    /// `scheduler_inbound` — 调度器的入站发送端
    /// `scheduler_outbound` — 调度器的出站接收端
    pub fn bridge_to_scheduler(
        &mut self,
        scheduler_inbound: std::sync::mpsc::Sender<tremolite_core::scheduler::SessionTask>,
        scheduler_outbound: std::sync::mpsc::Receiver<tremolite_message::OutboundMessage>,
    ) {
        if self.bridged {
            tracing::warn!("channels: bridge already active, skipping");
            return;
        }

        // 1. 取出 tokio 入站接收端
        let tokio_rx = self.registry.take_rx()
            .expect("bridge_to_scheduler called twice: take_rx already consumed");

        // 2. 将 registry 移入 Arc 供桥线程和工具共用
        let shared_registry = Arc::new(TokioMutex::new(
            std::mem::replace(&mut self.registry, ChannelRegistry::new())
        ));
        self.bridge_registry = Some(shared_registry.clone());

        // 3. 工具发消息的出站通道
        let (tool_tx, mut tool_rx) = tokio::sync::mpsc::channel::<OutboundMessage>(256);
        self.outbound_tool_tx = Some(tool_tx);

        // 4. 入站桥线程：tokio rx → scheduler inbound_tx
        let inbound_tx = scheduler_inbound;
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("bridge: failed to create tokio runtime for inbound");
            rt.block_on(async move {
                let mut rx = tokio_rx;
                while let Some(inbound) = rx.recv().await {
                    use tremolite_core::scheduler::SessionTask;
                    let task = SessionTask {
                        session_id: inbound.sender.clone(),
                        input: inbound.content,
                        channel: inbound.channel,
                        sender: inbound.sender,
                    };
                    if inbound_tx.send(task).is_err() {
                        tracing::info!("bridge: scheduler dropped, stopping inbound bridge");
                        break;
                    }
                }
                tracing::info!("bridge: inbound bridge stopped");
            });
        });

        // 5. 出站桥线程：scheduler outbound_rx + tool_tx → channel.send()
        let reg = shared_registry.clone();
        thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new()
                .expect("bridge: failed to create tokio runtime for outbound");
            loop {
                // 监听调度器出站（有超时，以便同时检查 tool_rx）
                let from_scheduler = scheduler_outbound.recv_timeout(Duration::from_millis(200));
                match from_scheduler {
                    Ok(msg) => {
                        tracing::debug!(
                            "bridge: outbound msg for channel '{}', target '{}': {} chars",
                            msg.channel, msg.target, msg.content.len()
                        );
                        let result = rt.block_on(async {
                            let registry = reg.lock().await;
                            registry.send(&msg).await
                        });
                        if let Err(e) = result {
                            tracing::error!("bridge: outbound send error: {}", e);
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // 用超时而不是死等，给 tool 消息机会
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
                // 检查工具发来的消息
                while let Ok(msg) = tool_rx.try_recv() {
                    let result = rt.block_on(async {
                        let registry = reg.lock().await;
                        registry.send(&msg).await
                    });
                    if let Err(e) = result {
                        tracing::error!("bridge: tool outbound error: {}", e);
                    }
                }
            }
            tracing::info!("bridge: outbound bridge stopped");
        });

        self.bridged = true;
        tracing::info!("channels: bridged to scheduler");
    }
}

impl Default for ChannelsModule {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Module for ChannelsModule {
    fn id(&self) -> &str {
        "channels"
    }

    fn name(&self) -> &str {
        "消息通道"
    }

    fn version(&self) -> &str {
        "0.1.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "channels.qqbot".into(),
            "channels.napcat".into(),
            "channels.http".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec![]
    }

    fn required_modules(&self) -> Vec<&str> {
        vec![]
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "list_channels".into(),
                    description: "列出所有已注册的消息通道及其状态".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "send_message".into(),
                    description: "通过指定通道发送消息到外部平台".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "channel": { "type": "string", "description": "目标通道名" },
                            "target": { "type": "string", "description": "目标标识，如 group:123456 或 private:654321" },
                            "content": { "type": "string", "description": "消息内容" },
                        },
                        "required": ["channel", "target", "content"],
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "list_channels" => {
                let names = self.list_channels();
                if names.is_empty() {
                    Ok("没有已注册的消息通道".into())
                } else {
                    Ok(format!("已注册通道: {}", names.join(", ")))
                }
            }
            "send_message" => {
                let parsed: std::collections::HashMap<String, serde_json::Value> =
                    serde_json::from_str(args)
                        .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析: {e}")))?;
                let channel = parsed.get("channel")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let target = parsed.get("target")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = parsed.get("content")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if channel.is_empty() || target.is_empty() || content.is_empty() {
                    return Err(ModuleError::ToolExecutionFailed(
                        "缺少必要参数：channel/target/content".into(),
                    ));
                }

                // 走桥线程的出站通道发送（桥线程有 tokio runtime 处理异步）
                let msg = OutboundMessage::new(content, channel, target);
                if let Some(ref tx) = self.outbound_tool_tx {
                    tx.try_send(msg)
                        .map_err(|e| ModuleError::ToolExecutionFailed(format!("发送失败: {e}")))?;
                    Ok(format!("消息已发送到通道 '{}'，目标 '{}'", channel, target))
                } else {
                    // 未桥接时尝试直接用 registry 发（需要 tokio runtime）
                    let msg = OutboundMessage::new(content, channel, target);
                    let registry_send = self.registry.send(&msg);
                    tokio::runtime::Handle::try_current()
                        .map_err(|_| ModuleError::ToolExecutionFailed("桥未启动且无 tokio 运行时".into()))
                        .and_then(|_| {
                            tokio::task::block_in_place(|| {
                                tokio::runtime::Handle::current()
                                    .block_on(registry_send)
                                    .map_err(|e| ModuleError::ToolExecutionFailed(e))
                                    .map(|_| format!("消息已发送到通道 '{}'，目标 '{}'", channel, target))
                            })
                        })
                }
            }
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn display_status(&self) -> Option<String> {
        if self.channel_names.is_empty() {
            Some("通道: 无".into())
        } else {
            Some(format!("通道: {}", self.channel_names.join(", ")))
        }
    }

    fn on_event(&mut self, event: &Event, ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                tracing::info!("channels: module ready ({} channels)", self.channel_names.len());
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                tracing::info!("channels: shutting down all channels");
                // Ideally we'd await shutdown here, but on_event is sync
                // The server handles the actual channel shutdown at exit
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
