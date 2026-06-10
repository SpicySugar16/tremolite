//! ## 模块通讯协议核心类型
//!
//! 定义模块间通讯的标准信封、引擎服务接口、模块声明和健康等共享数据结构。
//!
//! 这里没有「模块之间怎么通信」——模块只通过 PowerCoupling 接入引擎，
//! 通过引擎获取所有服务。模块之间不直接对话。

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex};

use serde::{Deserialize, Serialize};

// ─── 引擎服务描述 ────────────────────────────────

/// 引擎暴露给模块的服务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDefinition {
    /// 服务名称，如 "emotion.detect", "memory.recall"
    pub name: String,
    /// 服务描述
    pub description: String,
}

// ─── 模块声明 ─────────────────────────────────────

/// 模块宣言——模块启动时向引擎声称的信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleDeclaration {
    /// 模块 ID（唯一标识）
    pub module_id: String,
    /// 人类可读名称
    pub name: String,
    /// 版本号（语义化）
    pub version: String,
    /// 作者信息
    pub author: ModuleAuthor,
    /// 本模块对外提供的服务列表
    #[serde(default)]
    pub provides: Vec<ServiceDefinition>,
    /// 本模块依赖的服务列表
    #[serde(default)]
    pub requires: Vec<ServiceDefinition>,
    /// 依赖的模块 ID 列表
    #[serde(default)]
    pub required_modules: Vec<String>,
    /// 本模块感兴趣的事件类型
    #[serde(default)]
    pub handlers: Vec<String>,
}

/// 模块作者声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleAuthor {
    /// 作者名（必填）
    pub name: String,
    /// 联系方式（必填，GitHub 链接、邮件等）
    pub contact: String,
    /// 模块用途描述（必填）
    pub description: String,
    /// 许可证（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// 签名（可选）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

// ─── 标准消息信封 ────────────────────────────────

/// 引擎内部消息——模块通过 PowerCoupling 发送给引擎，
/// 引擎根据服务名称路由到对应的处理器。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleMessage {
    /// 消息唯一 ID（UUID v4 字符串）
    pub id: String,
    /// 发送方模块 ID
    pub from: String,
    /// 路由目标：服务名称（如 "emotion.detect"）或事件名（如 "Startup"）
    pub to: String,
    /// 消息类型
    pub kind: MessageKind,
    /// 消息头
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// 消息负载（JSON）
    pub payload: serde_json::Value,
    /// 回复目标消息 ID（如果是对某个请求的回复）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// 时间戳（Unix 毫秒）
    pub timestamp: u64,
}

impl ModuleMessage {
    /// 创建一条请求消息
    pub fn request(from: &str, service: &str, payload: serde_json::Value) -> Self {
        Self {
            id: uuid_v4(),
            from: from.to_string(),
            to: service.to_string(),
            kind: MessageKind::Request,
            headers: HashMap::new(),
            payload,
            reply_to: None,
            timestamp: now_millis(),
        }
    }

    /// 创建对某条消息的回复
    pub fn response(from: &str, to: &str, payload: serde_json::Value, reply_to: &str) -> Self {
        Self {
            id: uuid_v4(),
            from: from.to_string(),
            to: to.to_string(),
            kind: MessageKind::Response,
            headers: HashMap::new(),
            payload,
            reply_to: Some(reply_to.to_string()),
            timestamp: now_millis(),
        }
    }

    /// 创建一条广播消息
    pub fn broadcast(from: &str, topic: &str, payload: serde_json::Value) -> Self {
        Self {
            id: uuid_v4(),
            from: from.to_string(),
            to: topic.to_string(),
            kind: MessageKind::Broadcast,
            headers: HashMap::new(),
            payload,
            reply_to: None,
            timestamp: now_millis(),
        }
    }

    /// 创建一条事件消息（引擎驱动模块）
    pub fn event(from: &str, event_name: &str, payload: serde_json::Value) -> Self {
        Self {
            id: uuid_v4(),
            from: from.to_string(),
            to: event_name.to_string(),
            kind: MessageKind::Event,
            headers: HashMap::new(),
            payload,
            reply_to: None,
            timestamp: now_millis(),
        }
    }

    /// 创建一条声明消息（模块注册时使用）
    pub fn declaration(from: &str, payload: serde_json::Value) -> Self {
        Self {
            id: uuid_v4(),
            from: from.to_string(),
            to: "engine".to_string(),
            kind: MessageKind::Declaration,
            headers: HashMap::new(),
            payload,
            reply_to: None,
            timestamp: now_millis(),
        }
    }

    /// 创建一条错误回复
    pub fn error(from: &str, to: &str, code: &str, message: &str, reply_to: &str) -> Self {
        Self {
            id: uuid_v4(),
            from: from.to_string(),
            to: to.to_string(),
            kind: MessageKind::Error,
            headers: HashMap::new(),
            payload: serde_json::json!({ "code": code, "message": message }),
            reply_to: Some(reply_to.to_string()),
            timestamp: now_millis(),
        }
    }

    pub fn is_request(&self) -> bool { matches!(self.kind, MessageKind::Request) }
    pub fn is_error(&self) -> bool { matches!(self.kind, MessageKind::Error) }
}

/// 消息类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MessageKind {
    /// 请求——发送方期望回复
    Request,
    /// 回复——对某个请求的响应
    Response,
    /// 广播
    Broadcast,
    /// 事件——引擎生命周期驱动模块
    Event,
    /// 声明——模块注册时告知引擎身份
    Declaration,
    /// 错误
    Error,
}

// ─── 模块健康 ─────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleHealth {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: ModuleStatus,
    pub message_count: u64,
    pub error_count: u64,
    pub uptime_secs: u64,
    pub services: Vec<String>,
    pub dependencies: Vec<String>,
    pub last_error: Option<String>,
    pub details: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ModuleStatus {
    Running,
    Degraded,
    Error,
    Stopped,
}

impl Default for ModuleStatus { fn default() -> Self { ModuleStatus::Running } }

impl std::fmt::Display for ModuleStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleStatus::Running => write!(f, "running"),
            ModuleStatus::Degraded => write!(f, "degraded"),
            ModuleStatus::Error => write!(f, "error"),
            ModuleStatus::Stopped => write!(f, "stopped"),
        }
    }
}

// ─── PowerCoupling ──────────────────────────────

/// 动力耦合器——模块接入引擎的"液压接头"
///
/// 每个模块在注册后获得一个 PowerCoupling 实例。
/// 通过它调用引擎提供的各种服务（LLM、记忆、情绪检测等），
/// 不需要知道服务由哪个模块实现。
///
/// 就像挖掘机的铲斗——插上快换接头，就能用引擎的液压动力，
/// 不需要知道油泵在哪。
#[derive(Clone)]
pub struct PowerCoupling {
    module_id: String,
    tx: mpsc::Sender<ModuleMessage>,
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<ModuleMessage>>>>,
}

impl std::fmt::Debug for PowerCoupling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PowerCoupling")
            .field("module_id", &self.module_id)
            .finish()
    }
}

impl PowerCoupling {
    /// 创建新的动力耦合器
    pub fn new(
        module_id: &str,
        tx: mpsc::Sender<ModuleMessage>,
    ) -> Self {
        Self {
            module_id: module_id.to_string(),
            tx,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn module_id(&self) -> &str {
        &self.module_id
    }

    /// 调用引擎服务并等待回复（同步阻塞）
    ///
    /// `service`——服务名称，如 "emotion.detect", "memory.recall"
    /// `payload`——调用参数（JSON）
    /// `timeout_secs`——超时秒数
    pub fn call(&self, service: &str, payload: serde_json::Value, timeout_secs: u64)
        -> Result<serde_json::Value, CouplingError>
    {
        let msg = ModuleMessage::request(&self.module_id, service, payload);
        let id = msg.id.clone();

        // 注册等待
        let (done_tx, done_rx) = mpsc::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| CouplingError::Disconnected)?;
            pending.insert(id.clone(), done_tx);
        }

        // 发送请求
        self.tx.send(msg).map_err(|_| CouplingError::Disconnected)?;

        // 等待回复
        match done_rx.recv_timeout(std::time::Duration::from_secs(timeout_secs)) {
            Ok(response) => {
                if response.is_error() {
                    let code = response.payload["code"].as_str().unwrap_or("UNKNOWN");
                    let msg = response.payload["message"].as_str().unwrap_or("no message");
                    Err(CouplingError::ServiceError(format!("{}: {}", code, msg)))
                } else {
                    Ok(response.payload)
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut pending = self.pending.lock().map_err(|_| CouplingError::Disconnected)?;
                pending.remove(&id);
                Err(CouplingError::Timeout(timeout_secs))
            }
            Err(_) => Err(CouplingError::Disconnected),
        }
    }

    /// 调用引擎服务不等待回复（fire-and-forget）
    pub fn fire(&self, service: &str, payload: serde_json::Value) -> Result<(), CouplingError> {
        let msg = ModuleMessage {
            id: uuid_v4(),
            from: self.module_id.clone(),
            to: service.to_string(),
            kind: MessageKind::Request,
            headers: HashMap::new(),
            payload,
            reply_to: None,
            timestamp: now_millis(),
        };
        self.tx.send(msg).map_err(|_| CouplingError::Disconnected)
    }

    /// 广播消息给其他模块
    pub fn broadcast(&self, topic: &str, payload: serde_json::Value) -> Result<(), CouplingError> {
        let msg = ModuleMessage::broadcast(&self.module_id, topic, payload);
        self.tx.send(msg).map_err(|_| CouplingError::Disconnected)
    }

    /// 发送声明
    pub fn declare(&self, declaration: ModuleDeclaration) -> Result<(), CouplingError> {
        let payload = serde_json::to_value(declaration)
            .map_err(|e| CouplingError::Protocol(format!("serialize: {}", e)))?;
        let msg = ModuleMessage::declaration(&self.module_id, payload);
        self.tx.send(msg).map_err(|_| CouplingError::Disconnected)
    }

    /// 引擎投递回复到等待队列
    pub fn deliver(&self, msg: ModuleMessage) -> Result<(), CouplingError> {
        let reply_to = msg.reply_to.as_ref()
            .ok_or_else(|| CouplingError::Protocol("response missing reply_to".into()))?;
        let mut pending = self.pending.lock().map_err(|_| CouplingError::Disconnected)?;
        if let Some(tx) = pending.remove(reply_to) {
            let _ = tx.send(msg);
        }
        Ok(())
    }

    /// 向引擎发送事件（模块想触发某个引擎事件时使用）
    pub fn emit(&self, event_name: &str, payload: serde_json::Value) -> Result<(), CouplingError> {
        let msg = ModuleMessage::event(&self.module_id, event_name, payload);
        self.tx.send(msg).map_err(|_| CouplingError::Disconnected)
    }
}

// ─── 耦合器错误 ─────────────────────────────────

#[derive(Debug, Clone, thiserror::Error)]
pub enum CouplingError {
    #[error("service call timed out after {0}s")]
    Timeout(u64),
    #[error("service returned error: {0}")]
    ServiceError(String),
    #[error("coupling disconnected (engine shutting down)")]
    Disconnected,
    #[error("protocol error: {0}")]
    Protocol(String),
}

// ─── 模块收到的消息事件 ──────────────────────────

/// 模块收到的消息事件
#[derive(Debug, Clone)]
pub enum MessageEvent {
    Request(ModuleMessage),
    Response(ModuleMessage),
}

impl MessageEvent {
    pub fn message(&self) -> &ModuleMessage {
        match self {
            MessageEvent::Request(m) | MessageEvent::Response(m) => m,
        }
    }
}

// ─── 辅助函数 ────────────────────────────────────

fn uuid_v4() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let rand_part: u64 = rand::random();
    format!("{:016x}{:016x}", now, rand_part)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
