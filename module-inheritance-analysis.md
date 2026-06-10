# 缺失能力归属分析

按现有模块体系看，七个能力落在三种位置：**改现有模块**、**加新 Module**、**加新 crate（非 Module）**。

---

## 一、继承进现有模块的

### 1. 会话隔离 — 改全部 5 个现有 Module

现有模块全是全局单例状态。EmotionModule 一个 `state` 管所有对话，MemoryModule 一个 `manager` 管所有对话。这是错的。

改造方案：每个 Module 的 `on_event()` 接收 `EventContext`，里面加 `session_id`。模块内部按 session_id 分叉：

```
EmotionModule {
    states: HashMap<String, EmotionState>,  // session_id -> state
    current: String,                         // 当前活跃 session
}
```

MemoryModule、LearningModule、AttentionModule、PlanModule 同理。session_id 为空时走「默认会话」，向后兼容。

**涉及改动：**
- `Event` 枚举加 `session_id` 字段（或放在 `EventContext` 里）
- 5 个模块各自加 `HashMap<session_id, InnerState>` 
- TTL 清理回调（由 SessionManager 调用）

### 2. 反思去毒 — MemoryModule 执行，ReflectionModule 触发

去毒的实操（虚构检测、噪音删除、去重）本质是操作记忆库底层数据，归 MemoryModule。ReflectionModule 只负责定期触发和结果记录。

**方案：** MemoryModule 加 `fn decontaminate()` 方法，直接操作自己的 `manager`。Event 枚举加 `Decontaminate` 事件。ReflectionModule 计数满 N 条后广播 `Event::Decontaminate` → MemoryModule 的 `on_event` 响应执行。

### 3. 技能系统 — LearningModule 并入 SkillModule

技能加载、熟练度追踪、工具选择策略、遗忘曲线全是同一件事。学习引擎是技能系统内部的运作机制，不是平行模块。直接改名合并。

**方案：** `modules/learning.rs` 改名 `modules/skill.rs`，`LearningModule` 改名 `SkillModule`。保留全部学习引擎能力。加 `fn inject_skill(&mut self, path: &str)`。`BuildPrompt` 事件时按熟练度筛选注入。`list_skills` / `view_skill` 工具也注册在同一个模块里。

---

## 二、需要新建 Module（实现 Module trait）

### 4. 消息通道 — 每个通道一个 Module

消息通道有明确的「接收外来消息→触发引擎处理→发回响应」的生命周期，天然适配 Module trait 的事件模型。

**方案：** 每个通道实现一个 `Module`：
- `QQBotModule` — 连 QQ 开放平台 API
- `NapCatModule` — WebSocket 连 NapCat
- `WebhookModule` — 接收 HTTP POST
- 注册后 `on_event(Startup)` 里启动后台连接
- `on_event(OnToolCall)` 处理 `send_message` 工具调用
- 收到外部消息时通过 `EventContext` 回写引擎

**好处：** 模块系统已经有的事件广播、工具注册、生命周期管理，通道模块直接用，不用新造轮子。

### 5. 定时任务 — CronModule（或直接跑后台任务）

`tremolite-cron` crate 有调度器骨架。接入方式两种：
- 做一个 `CronModule` 注册进引擎，`on_event(Startup)` 启动调度器，`on_event(Shutdown)` 停止
- 或者引擎 `run()` 里 spawn 一个独立 tokio 任务

**推荐前者**——统一生命周期管理。

### 6. 反思系统核心 — ReflectionModule

反思的「定期触发」不适合塞进 MemoryModule（职责分离）。做一个 `ReflectionModule`：
- `on_event(OnResponse)` 时计数，满 N 条触发反思
- 调用 LLM 做摘要 + 重要性评分
- 写回 MemoryModule（通过 EventContext 的 engine handle 访问）

### 7. 技能系统 — SkillModule

技能文件的加载、检索、注入，独立出一个 `SkillModule`：
- `on_event(BuildPrompt)` 时根据上下文选择技能注入 system prompt
- 提供 `list_skills` / `view_skill` 工具
- 跟 LearningModule 通过 `EventContext` 交互（学习记录）

---

## 三、需要新建 crate（不实现 Module trait）

### 8. 会话管理 — `tremolite-session` crate

SessionManager 不实现 Module trait。它是引擎层面的基础设施，在引擎 `run()` 之前初始化，作为 TremoliteEngine 的一个字段存在。

**原因：** 会话管理不是「可插拔的功能模块」，它修改了所有模块的运行方式。作为引擎基础层更合理。

### 9. MCP 客户端 — `tremolite-mcp` crate

MCP 是纯工具类功能：连接外部服务、发现工具、调用工具。不参与事件循环，不贡献 prompt。

**方案：** 独立 crate，提供 `MCPClient` struct。在引擎初始化时加载配置，工具注册到 `tool_executor` / `ToolRegistry`。不实现 Module trait——MCP 是工具链的扩展，不是引擎的模块。

### 10. 任务委派 — `tremolite-delegate` crate

子进程 spawn + 通信协议。作为独立 crate，DelegateModule 可以做成 Module（处理委派工具调用），但子进程通信本身是基础设施。

**方案：** `tremolite-delegate` crate 提供子进程管理和 JSON 行协议。`DelegateModule`（可选 Module）暴露 `delegate_task` 工具给 LLM。

---

## 总表

| 能力 | 归属 | 类型 |
|------|------|------|
| 会话隔离 | 5 个现有 Module 改造 | 改现有 |
| 反思去毒 | MemoryModule 扩展 | 改现有 |
| 技能系统 | SkillModule（原 LearningModule 改名） | 改现有 |
| 消息通道 | QQBotModule / NapCatModule 等 | 新建 Module |
| 定时任务 | CronModule | 新建 Module |
| 反思引擎 | ReflectionModule | 新建 Module |
| 会话管理 | `tremolite-session` crate | 新建 crate |
| MCP 客户端 | `tremolite-mcp` crate | 新建 crate |
| 任务委派 | `tremolite-delegate` crate | 新建 crate |
