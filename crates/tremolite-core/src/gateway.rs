use std::collections::HashMap;
use std::sync::mpsc;

// ─── 消息类型（复用 tremolite-message 定义）───────────

pub use tremolite_message::{InboundMessage, OutboundMessage};

// ─── Gateway trait ────────────────────────────────

/// 通道的抽象接口
/// 每个通道（CLI、QQ、Telegram 等）实现此 trait
pub trait Gateway: Send {
    /// 通道名称
    fn name(&self) -> &str;

    /// 发送消息到指定目标
    fn send(&self, msg: &OutboundMessage) -> Result<(), String>;

    /// 启动接收循环
    /// 通过 sender 把收到的消息发出去
    fn start(&self, sender: mpsc::Sender<InboundMessage>) -> Result<(), String>;

    /// 停止接收循环
    fn stop(&self) -> Result<(), String>;
}

// ─── CLI Gateway ─────────────────────────────────

/// CLI 通道——通过标准输入输出交互
pub struct CliGateway {
    name: String,
    running: std::sync::atomic::AtomicBool,
}

impl CliGateway {
    pub fn new() -> Self {
        Self {
            name: "cli".into(),
            running: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Gateway for CliGateway {
    fn name(&self) -> &str { &self.name }

    fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        println!("{}", msg.content);
        Ok(())
    }

    fn start(&self, sender: mpsc::Sender<InboundMessage>) -> Result<(), String> {
        self.running.store(true, std::sync::atomic::Ordering::Relaxed);

        let name = self.name.clone();
        std::thread::spawn(move || {
            let mut input = String::new();
            while std::io::stdin().read_line(&mut input).is_ok() {
                let trimmed = input.trim().to_string();
                if trimmed.is_empty() {
                    input.clear();
                    continue;
                }
                let msg = InboundMessage::new(&trimmed, &name, "user");
                if sender.send(msg).is_err() {
                    break;
                }
                input.clear();
            }
        });

        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        self.running.store(false, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }
}

// ─── Null Gateway ────────────────────────────────

/// 空通道——用于测试
pub struct NullGateway {
    name: String,
    pub sent_messages: std::sync::Mutex<Vec<OutboundMessage>>,
}

impl NullGateway {
    pub fn new() -> Self {
        Self {
            name: "null".into(),
            sent_messages: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Gateway for NullGateway {
    fn name(&self) -> &str { &self.name }

    fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        if let Ok(mut msgs) = self.sent_messages.lock() {
            msgs.push(msg.clone());
        }
        Ok(())
    }

    fn start(&self, _sender: mpsc::Sender<InboundMessage>) -> Result<(), String> {
        Ok(())
    }

    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
}

// ─── 通道路由器 ─────────────────────────────────

/// 通道路由器——消息中心
/// 管理所有注册的通道，路由消息
pub struct GatewayRouter {
    gateways: HashMap<String, Box<dyn Gateway>>,
    inbox: mpsc::Receiver<InboundMessage>,
    outbox_tx: mpsc::Sender<InboundMessage>,
}

impl GatewayRouter {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            gateways: HashMap::new(),
            inbox: rx,
            outbox_tx: tx,
        }
    }

    /// 注册一个通道
    pub fn register(&mut self, gateway: Box<dyn Gateway>) -> Result<(), String> {
        let name = gateway.name().to_string();
        gateway.start(self.outbox_tx.clone())?;
        self.gateways.insert(name, gateway);
        Ok(())
    }

    /// 获取一个通道
    pub fn get(&self, name: &str) -> Option<&dyn Gateway> {
        self.gateways.get(name).map(|g| g.as_ref())
    }

    /// 向指定通道发送消息
    pub fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        if let Some(gateway) = self.gateways.get(&msg.channel) {
            gateway.send(msg)
        } else {
            Err(format!("Gateway '{}' not found", msg.channel))
        }
    }

    /// 广播给所有通道
    pub fn broadcast(&self, content: &str, source_channel: &str) {
        for (name, gateway) in &self.gateways {
            if name != source_channel {
                let msg = OutboundMessage::new(content, name, "all");
                let _ = gateway.send(&msg);
            }
        }
    }

    /// 接收一条入站消息（阻塞）
    pub fn recv(&self) -> Result<InboundMessage, String> {
        self.inbox.recv().map_err(|e| e.to_string())
    }

    /// 尝试接收一条（非阻塞）
    pub fn try_recv(&self) -> Result<InboundMessage, String> {
        self.inbox.try_recv().map_err(|e| e.to_string())
    }

    /// 列出所有注册的通道
    pub fn list_channels(&self) -> Vec<String> {
        self.gateways.keys().cloned().collect()
    }
}

// ─── 单元测试 ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_gateway() {
        let gateway = NullGateway::new();
        assert_eq!(gateway.name(), "null");

        let msg = OutboundMessage::new("你好", "null", "user");
        assert!(gateway.send(&msg).is_ok());

        let msgs = gateway.sent_messages.lock().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "你好");
    }

    #[test]
    fn test_router_register() {
        let mut router = GatewayRouter::new();
        assert!(router.register(Box::new(NullGateway::new())).is_ok());
        let channels = router.list_channels();
        assert!(channels.contains(&"null".into()));
    }

    #[test]
    fn test_message_constructors() {
        let inbound = InboundMessage::new("神大人好~", "cli", "user");
        assert_eq!(inbound.content, "神大人好~");
        assert_eq!(inbound.channel, "cli");

        let outbound = OutboundMessage::new("噜噜……", "cli", "user");
        assert_eq!(outbound.content, "噜噜……");
    }

    #[test]
    fn test_router_send() {
        let mut router = GatewayRouter::new();
        let null_gw = NullGateway::new();
        router.register(Box::new(null_gw)).unwrap();

        let msg = OutboundMessage::new("test", "null", "user");
        assert!(router.send(&msg).is_ok());
    }

    #[test]
    fn test_unknown_channel() {
        let router = GatewayRouter::new();
        let msg = OutboundMessage::new("test", "nonexistent", "user");
        assert!(router.send(&msg).is_err());
    }
}
