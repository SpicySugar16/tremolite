# Session 调度器骨架方案

## 一、现状问题

```
TremoliteEngine.run()
  └─ loop {
      recv()            ← 阻塞，只等一个消息
      broadcast()       ← 模块处理（单 session_id）
      process_with_llm()← 阻塞，跑完整轮 LLM + 工具链
      send()            ← 发完回来收下一条
  }
```

**瓶颈：** 整条链路是单线程的。A session 的 LLM 请求还没回来，B session 的消息就堵在队列里。cron job 也一样——没地方插进去跑。

## 二、目标架构

```
  GatewayRouter.recv()  →  InboundQueue
                                 │
                    SessionScheduler (调度器)
                      /      |       \
                     /       |        \
              SessionWorker  SessionWorker  CronWorker
              (thread)       (thread)       (thread)
                     \       |        /
                      \      |       /
                     OutboundQueue → GatewayRouter.send()
```

- **InboundQueue：** 收消息队列，不去重不排序，只按 session_id 分组
- **SessionScheduler：** 调度线程，从 InboundQueue 取消息，按 session_id 投递到对应 Worker
- **SessionWorker：** 每个活跃 session 一个独立工作线程，跑完整的话 turn（broadcast → LLM → send）
- **CronWorker：** cron job 作为特殊 session 接入调度器，触发源是定时器不是消息
- **OutboundQueue：** 所有 Worker 的回复写入队列，调度器负责回写给 GatewayRouter

## 三、模块拆分

### tremolite-scheduler（新增）

```rust
pub struct SessionScheduler {
    /// 活跃的 session 工作线程池
    workers: Arc<Mutex<HashMap<String, SessionWorkerHandle>>>,
    /// 线程池（rayon 或自定义）
    pool: ThreadPool,
    /// 入站队列
    inbound: crossbeam::channel::Sender<SessionTask>,
    /// 出站队列
    outbound: crossbeam::channel::Receiver<OutboundMessage>,
}

pub struct SessionTask {
    pub session_id: String,
    pub input: String,
    pub channel: String,
    pub sender: String,
}

pub struct SessionWorkerHandle {
    pub session_id: String,
    pub last_active: u64,
    pub status: WorkerStatus,  // Active | Idle | Cooling
}
```

### SessionWorker（调度器内部）

每个 Worker 拥有一份运行所需的状态副本：
- `Arc<ProviderRegistry>`（线程安全，共享引用）
- `ModuleRegistry` 的每个模块需要 `Send + Sync` 或按 session 分叉副本
- 各自的 `session_id` 和 `base_soul`

Worker 的生命周期：
1. 调度器创建 Worker → 广播 Startup 事件（传入 session_id）
2. Worker 进入消息等待循环
3. 收到消息 → 广播 OnMessage → BuildPrompt → process_with_llm → OnResponse → send
4. 超时无消息 → 状态设为 Idle，不退出但不再占用线程资源
5. 调度器可回收 Idle Worker 的资源

## 四、需要改动的地方

### 1. `TremoliteEngine` 改造
- `run()` 改为启动调度器后不阻塞，只做事件监听
- `process_with_llm()` 改为接受 `session_id` 参数，不依赖 `&mut self`（或改用内部可变）
- `tool_executor` 改为 `Arc<dyn ToolExecutor + Send + Sync>`

### 2. `ModuleRegistry` 与模块并发
- 目前模块通过 `EngineHandle`（内部 `Weak<Mutex<...>>`）访问，锁在模块内
- 核心问题是 `Module::execute_tool` 同时只允许一个调用者——每个 Worker 需要独立的 module 实例，或模块改为无锁设计
- **最小改动方案：** Worker 创建时 clone 模块注册表，每个 session 有自己独立的模块实例（memory/session/emotion 等按 session_id 隔离）

### 3. `ToolCallLoop` + `ToolExecutor`
- `ToolCallLoop::run()` 已经是 `&self`，可以共享调用
- `ToolExecutor` 需要 `Send + Sync`（目前 `CompositeToolExecutor` 内部持有 `ModuleRegistry`，需要按 session 做隔离）

### 4. 模块 session 隔离
- `MemoryModule` 已支持按 session_id 分片——每个 Worker 用不同 session_id 即可
- `SessionModule` 已支持多 session 管理
- `ReflectionModule` 当前是全局触发（按消息计数），改为按 session 独立触发
- `EmotionModule` 当前是全局状态，改为按 session 持有独立情绪实例

## 五、实施步骤

### Phase 1 — 线程安全改造
- `ToolExecutor` trait 加 `Send + Sync` bound
- `CompositeToolExecutor` 内部 `ModuleRegistry` 改为 `Arc<Mutex<ModuleRegistry>>`
- `process_with_llm` 改为接受 `&self` + `session_id` 参数（用内部锁处理可变部分）
- 确认所有模块 trait 方法在并发调用下不 panic

### Phase 2 — 调度器核心
- 新建 `tremolite-scheduler` crate，实现 `SessionScheduler`
- 实现 `SessionWorker` 的基本消息循环
- 用 `crossbeam` channel 做线程间通信
- Worker 创建时 clone ProviderRegistry（Arc 增计数，零拷贝）

### Phase 3 — Engine 对接
- `TremoliteEngine::run()` 改为启动 Scheduler 后进入轻量事件循环
- 模块广播改为通过 Worker 内部调用（不再走 engine 主线程）
- 迁移 cron job 到调度器作为定时 session

### Phase 4 — 淘汰与弹性
- Worker Idle 超时回收
- 调度器根据入站队列积压动态调整 Worker 数量
- 单个 Worker crash 不影响其他 session
