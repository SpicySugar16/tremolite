# 透闪石模块接口协议 v2

> 引擎不是交换机，是**挖掘机臂**。
> 模块不是通信对等体，是**铲斗/破碎锤/抓木器**——通过标准的液压接头接入引擎，用引擎的动力干活。

---

## 一、核心隐喻

```
  ┌──────────────────────────────────────────────┐
  │              引擎（挖掘机机体）                  │
  │  ┌──────┐  ┌────────┐  ┌───────┐  ┌──────┐  │
  │  │ LLM  │  │ 调度器  │  │ Prompt│  │ 会话  │  │
  │  │ 引擎  │  │        │  │ 构建器 │  │ 管理  │  │
  │  └──┬───┘  └───┬────┘  └───┬───┘  └──┬───┘  │
  │     │          │           │         │       │
  │     └──────────┴───────────┴─────────┘       │
  │            标准快换接头（PowerCoupling）         │
  └──────────────────┬───────────────────────────┘
                     │
         ┌───────────┼───────────┐
         │           │           │
    ┌────▼───┐  ┌───▼────┐  ┌──▼─────┐
    │ 铲斗    │  │ 破碎锤  │  │ 抓木器  │
    │情绪模块  │  │压缩模块  │  │记忆模块  │
    └────────┘  └────────┘  └────────┘
```

- **引擎提供动力（Power Services）**：LLM 调用、会话调度、工具执行、Prompt 构建
- **模块接入引擎获取动力**：通过 `PowerCoupling` 标准接口
- **模块之间不直接对话**：需要其他模块的数据时，通过引擎的服务接口获取
- **模块声明自己是"什么工具"**：通过 Declaration 广播自己的能力、作者、依赖

---

## 二、引擎提供的动力

引擎通过 `PowerCoupling` 向模块暴露以下核心服务：

| 服务名称 | 描述 | 接口形式 |
|---------|------|---------|
| `llm.chat` | 调用 LLM 生成回复 | `request("llm.chat", payload) -> response` |
| `llm.tool_loop` | 完整工具循环（含函数调用） | `request("llm.tool_loop", payload) -> response` |
| `session.dispatch` | 向会话调度器投递消息 | `send("session.dispatch", payload)` |
| `memory.recall` | 查询历史消息 | `request("memory.recall", payload) -> response` |
| `memory.store` | 存储消息 | `send("memory.store", payload)` |
| `emotion.detect` | 检测输入情绪 | `request("emotion.detect", payload) -> response` |
| `engine.config` | 查询引擎配置 | `request("engine.config", payload) -> response` |
| `engine.broadcast` | 广播给所有模块 | `send("engine.broadcast", payload)` |

模块通过 `PowerCoupling` 调用这些服务，就像铲斗通过液压管使用挖掘机的泵力一样——不需要知道油从哪来，只需要知道接头插上就能用。

---

## 三、PowerCoupling——模块接入引擎的标准接头

```rust
/// 动力耦合器——模块接入引擎的"液压接头"
///
/// 每个模块在注册后获得一个 PowerCoupling 实例。
/// 通过它调用引擎提供的各种服务，不需要知道服务由哪个模块实现。
#[derive(Clone)]
pub struct PowerCoupling {
    /// 本模块的 ID
    module_id: String,
    /// 向引擎发送消息的通道
    tx: mpsc::Sender<ModuleMessage>,
    /// 等待回复的挂起表（request_id → 回复通道）
    pending: Arc<Mutex<HashMap<String, mpsc::Sender<ModuleMessage>>>>,
}

impl PowerCoupling {
    /// 调用引擎服务并等待回复（同步阻塞，有超时）
    pub fn call(&self, service: &str, payload: Value, timeout_secs: u64) -> Result<Value, CouplingError>;

    /// 调用引擎服务不等待回复
    pub fn fire(&self, service: &str, payload: Value) -> Result<(), CouplingError>;

    /// 广播消息给其他模块
    pub fn broadcast(&self, topic: &str, payload: Value) -> Result<(), CouplingError>;
}
```

### 调用示例

```rust
// 情绪模块检测用户输入
fn on_event(&mut self, event: &Event, ctx: &EventContext) -> ... {
    if let Event::OnMessage { input, .. } = event {
        let result = self.coupling.call("emotion.detect", json!({"text": input}), 5)?;
        let emotion_label = result["label"].as_str().unwrap_or("neutral");
        // ...
    }
}

// 记忆模块存储消息
fn store(&self, session_id: &str, content: &str) {
    self.coupling.fire("memory.store", json!({
        "session_id": session_id,
        "content": content,
    })).ok();
}
```

注意：模块调用的是**服务名称**（如 `"emotion.detect"`），不是模块名称。引擎根据当前的模块注册情况，将服务请求路由到实际提供该服务的模块。如果以后换了情绪引擎的实现，服务名称不变。

---

## 四、模块声明与发现

每个模块在启动时向引擎发送 Declaration 消息，告知引擎：

1. **我是谁**：模块 ID、名称、版本
2. **谁写的**：作者名、联系方式、用途描述
3. **我能提供什么服务**：服务列表（对外暴露的能力）
4. **我需要什么**：依赖的服务列表
5. **我的信息类型**：我处理什么事件

```json
{
  "module_id": "emotion",
  "name": "情绪引擎",
  "version": "0.3.0",
  "author": {
    "name": "墨水仙猫",
    "contact": "https://github.com/spicysugar",
    "description": "透闪石情绪引擎——16 种复合情绪 + 5 级强度 + 风格注入",
    "license": "MIT"
  },
  "provides": [
    {"service": "emotion.detect", "description": "从文本检测情绪状态"},
    {"service": "emotion.style",   "description": "获取当前风格注入文本"}
  ],
  "requires": [],
  "handlers": ["Startup", "OnMessage", "Shutdown"]
}
```

---

## 五、模块间数据流动

### 5.1 模块调用另一个模块的服务

```
模块 A                 引擎                   模块 B
  │                     │                      │
  │──call("foo.do")─────│──────────────────────│
  │                     │──on_message(Request)─│
  │                     │                      │
  │                     │←───response──────────│
  │←────response────────│                      │
```

模块 A 不知道 foo.do 是谁实现的，它只通过引擎的耦合器发出请求。引擎根据服务注册表找到模块 B，转发请求，返回结果。

### 5.2 引擎驱动模块（事件）

```
引擎                         模块 C
  │                            │
  │──Event::Startup───────────│
  │  (通过 coupling 分发)      │ 模块初始化
  │                            │
  │──Event::OnMessage─────────│
  │  (通过 coupling 分发)      │ 模块处理输入
```

---

## 六、模块状态

```
┌──────────┐
│ Attached │  模块注册到引擎，耦合器已连接
└────┬─────┘
     │ 依赖检查通过 + 收到 Startup 事件
┌────▼─────┐
│  Running  │  正常提供服务
└────┬─────┘
     │ 收到 Shutdown 事件
┌────▼─────┐
│ Detached  │  耦合器断开，模块清理
└──────────┘
```

---

## 七、健康检查

引擎可以随时查询模块的健康状态。每个模块通过 `health()` 方法提供：

```rust
pub struct ModuleHealth {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: ModuleStatus,  // Running | Degraded | Error | Stopped
    pub message_count: u64,    // 处理过的消息数
    pub error_count: u64,      // 出错次数
    pub uptime_secs: u64,      // 运行时长
    pub services: Vec<String>, // 提供的服务
    pub dependencies: Vec<String>,  // 依赖的服务
    pub last_error: Option<String>,
    pub details: HashMap<String, String>,
}
```

---

## 八、向后兼容

旧模块（使用 `as_any()` + `downcast` 路径的）**不受影响**。新的 PowerCoupling 机制是附加能力，不是替代品：

- 旧模块：继续通过 `ModuleRegistry` 的 direct-access 路径工作
- 新模块：通过 `PowerCoupling` 工作
- 过渡模块：同时支持两者（通过 `set_coupling()` 获得耦合器，同时保留 old path）

引擎自动识别：
- 如果模块实现了 `declaration()` → 走新协议（通过耦合器路由服务请求）
- 如果模块没有 `declaration()` → 走旧路径（event 系统 + as_any downcast）

---

## 九、实施路径

1. 实现 `PowerCoupling` 类型和耦合器工厂
2. 在引擎中实现服务注册表（类比现有的能力索引）
3. 给 Module trait 加上 `set_coupling()` 和 `declaration()`
4. 迁移 EmotionModule 作为 PoC
5. 引擎主循环响应耦合器投递的请求，按服务名称路由
6. 逐步迁移其他模块
