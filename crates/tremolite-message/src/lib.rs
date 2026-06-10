use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

// ─── AoiMessage ────────────────────────────────────

/// Aoi 消息核心数据结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AoiMessage {
    /// 角色: "user" | "assistant" | "system" | "tool"
    pub role: String,
    /// 消息内容
    pub content: String,
    /// 可选的元数据
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

impl AoiMessage {
    pub fn new(role: &str, content: &str) -> Self {
        Self {
            role: role.to_string(),
            content: content.to_string(),
            metadata: None,
        }
    }

    pub fn user(content: &str) -> Self { Self::new("user", content) }
    pub fn assistant(content: &str) -> Self { Self::new("assistant", content) }
    pub fn system(content: &str) -> Self { Self::new("system", content) }
    pub fn tool(content: &str) -> Self { Self::new("tool", content) }
}

// ─── Inbound / Outbound 消息 ───────────────────────

/// 入站消息——从外部通道到引擎
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    pub content: String,
    pub channel: String,
    pub sender: String,
    pub metadata: HashMap<String, String>,
}

impl InboundMessage {
    pub fn new(content: &str, channel: &str, sender: &str) -> Self {
        Self {
            content: content.into(),
            channel: channel.into(),
            sender: sender.into(),
            metadata: HashMap::new(),
        }
    }
}

/// 出站消息——从引擎到外部通道
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundMessage {
    pub content: String,
    pub channel: String,
    pub target: String,
}

impl OutboundMessage {
    pub fn new(content: &str, channel: &str, target: &str) -> Self {
        Self {
            content: content.into(),
            channel: channel.into(),
            target: target.into(),
        }
    }
}

// ─── Channel trait (异步) ──────────────────────────

/// 消息通道抽象——每个外部消息平台实现此 trait
///
/// 生命周期：
/// 1. `name()` 返回通道标识
/// 2. `start()` 被引擎调用，传入 mpsc::Sender 用于向引擎发送收到的新消息
/// 3. 通道在后台 tokio task 中持续监听
/// 4. `send()` 被引擎调用来发送回复到该通道
/// 5. `stop()` 被引擎调用来关闭通道
#[async_trait::async_trait]
pub trait Channel: Send + Sync {
    /// 通道唯一标识，如 "http", "napcat", "qqbot"
    fn name(&self) -> &str;

    /// 启动通道的接收循环
    /// 引擎传入 sender——通道收到消息后通过它发回引擎
    async fn start(&self, sender: mpsc::Sender<InboundMessage>) -> Result<(), String>;

    /// 向该通道的目标发送消息
    async fn send(&self, msg: &OutboundMessage) -> Result<(), String>;

    /// 关闭通道
    async fn stop(&self) -> Result<(), String>;
}

// ─── ChannelRegistry ───────────────────────────────

/// 通道注册表——管理所有已注册的消息通道
///
/// 生命周期：
/// 1. 创建 new()
/// 2. 依次 register() 注册通道
/// 3. 桥接时 take_rx() 取出接收端
/// 4. 之后通过 send() 发送出站消息（&self 即可）
pub struct ChannelRegistry {
    pub(crate) channels: HashMap<String, Box<dyn Channel>>,
    /// 向引擎发送消息的通道（克隆给每个通道用）
    tx: Option<mpsc::Sender<InboundMessage>>,
    /// 从通道接收消息的收件箱（引擎端消费）
    rx: Option<mpsc::Receiver<InboundMessage>>,
}

impl ChannelRegistry {
    /// 创建新的通道注册表，同时创建 mpsc channel
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(256);
        Self {
            channels: HashMap::new(),
            tx: Some(tx),
            rx: Some(rx),
        }
    }

    /// 注册并启动一个通道
    pub async fn register(&mut self, channel: Box<dyn Channel>) -> Result<(), String> {
        let name = channel.name().to_string();
        let sender = self.tx.as_ref()
            .ok_or_else(|| "ChannelRegistry not initialized".to_string())?
            .clone();
        channel.start(sender).await?;
        self.channels.insert(name.clone(), channel);
        tracing::info!("channel: registered '{}'", &name);
        Ok(())
    }

    /// 获取收件箱 receiver（用于桥线程消费）
    pub fn take_rx(&mut self) -> Option<mpsc::Receiver<InboundMessage>> {
        self.rx.take()
    }

    /// 获取 tx 的克隆（供模块的 send_message 工具使用）
    pub fn tx_clone(&self) -> Option<mpsc::Sender<InboundMessage>> {
        self.tx.clone()
    }

    /// 向指定通道发送出站消息
    pub async fn send(&self, msg: &OutboundMessage) -> Result<(), String> {
        if let Some(channel) = self.channels.get(&msg.channel) {
            channel.send(msg).await
        } else {
            Err(format!("Channel '{}' not found", msg.channel))
        }
    }

    /// 广播给所有通道（排除来源通道）
    pub async fn broadcast(&self, content: &str, source_channel: &str) {
        for (name, channel) in &self.channels {
            if name != source_channel {
                let msg = OutboundMessage::new(content, name, "all");
                let _ = channel.send(&msg).await;
            }
        }
    }

    /// 关闭所有通道
    pub async fn shutdown(&self) {
        for (_name, channel) in &self.channels {
            let _ = channel.stop().await;
        }
    }

    /// 列出所有已注册的通道
    pub fn list_channels(&self) -> Vec<String> {
        self.channels.keys().cloned().collect()
    }

    /// 检查是否有指定名称的通道
    pub fn has_channel(&self, name: &str) -> bool {
        self.channels.contains_key(name)
    }

    /// 通道数量
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── 单元测试 ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct TestChannel;

    #[async_trait::async_trait]
    impl Channel for TestChannel {
        fn name(&self) -> &str { "test" }
        async fn start(&self, _sender: mpsc::Sender<InboundMessage>) -> Result<(), String> {
            Ok(())
        }
        async fn send(&self, _msg: &OutboundMessage) -> Result<(), String> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), String> { Ok(()) }
    }

    #[tokio::test]
    async fn test_channel_registry() {
        let mut registry = ChannelRegistry::new();
        assert!(registry.register(Box::new(TestChannel)).await.is_ok());
        assert!(registry.has_channel("test"));
        assert!(registry.take_rx().is_some());
    }

    #[test]
    fn test_aoi_message() {
        let msg = AoiMessage::new("user", "你好");
        assert_eq!(msg.role, "user");
        assert_eq!(msg.content, "你好");
        assert!(msg.metadata.is_none());
    }

    #[test]
    fn test_inbound_outbound() {
        let inbound = InboundMessage::new("hello", "cli", "user");
        assert_eq!(inbound.content, "hello");

        let outbound = OutboundMessage::new("world", "cli", "user");
        assert_eq!(outbound.content, "world");
    }
}
