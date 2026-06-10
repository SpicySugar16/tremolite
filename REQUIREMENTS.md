# 透闪石重构：需求说明书

> 本文档是对透闪石（Tremolite）当前架构的全面审计和重构需求描述。
> 目标：从「23 个 crate 拼在一起刚好能跑」变成「模块像盒子一样摆在架子上，改谁不动谁」。

---

## 一、现状 vs 目标

### 现状（图它什么）
- Module trait 干净，不枉统一
- 事件系统（Startup → OnMessage → BuildPrompt → OnResponse → Shutdown）完整
- 调度器多 session 隔离，通道抽象干净
- 架构的壳没问题，问题在壳里的布线

### 现状（疼在哪） — 六个痛点

#### 痛点 A：模块注册是硬编码的
`tremolite-cli/src/main.rs` 第 191-241 行，15 个 `register_module()` 调用一字排开。
想加一个模块？改代码。想关掉情绪引擎跑看看？改代码。想换记忆后端？改代码。
config.toml 认识 LLM provider、认识消息通道、认识 cron 任务，但不认识「要加载哪些模块」。

#### 痛点 B：模块依赖不被引擎理解
每个模块的 `required_modules()` 和 `requires()`（能力依赖）都写了，但 `ModuleRegistry` 注册时从不检查——它只是把模块插进 HashMap，然后不管了。
如果 A 依赖 B 而 B 没注册，不会报错。如果 A 和 B 互相依赖，不会检测环。
`order: Vec<String>` 是手动写的，没有拓扑排序。

#### 痛点 C：类型侵蚀 — 到处都是 downcast
`Module` trait 提供了 `as_any()` 和 `as_any_mut()`，然后没人用 trait。看看调度器的 `process()` 方法：

```rust
// 第 244-259 行：调度器直接 downcast MemoryModule
let mut history = self.modules.with_module("memory", |m| {
    m.as_any()
        .and_then(|any| any.downcast_ref::<crate::modules::memory::MemoryModule>())
        // ...
});

// 第 305-312 行：调度器直接 downcast SkillModule
self.modules.with_module_mut("skill", |m| {
    m.as_any_mut()
        .and_then(|any| any.downcast_mut::<crate::modules::skill::SkillModule>())
        // ...
});

// 第 320-326 行：调度器直接 downcast EmotionModule
self.modules.with_module("emotion", |m| {
    m.as_any()
        .and_then(|any| any.downcast_ref::<crate::modules::emotion::EmotionModule>())
        // ...
});
```

调度器不应该知道 `MemoryModule` 的模块内部结构。它应该通过**能力接口**要求「给我这个 session 的历史消息」，记忆模块应答。
同样的情况还出现在：`engine.create_scheduler()` 对 `DelegationModule` 和 `CronModule` 的专用注入路径（第 110-125 行）：
```rust
// 注：每个需要调度器通道的模块都要手动 downcast 注入
let _ = self.modules.with_module_mut("delegation", |m| {
    if let Some(dm) = m.as_any_mut()
        .and_then(|a| a.downcast_mut::<crate::modules::delegation::DelegationModule>())
    { dm.set_scheduler(...); }
});
let _ = self.modules.with_module_mut("cron", |m| {
    if let Some(cm) = m.as_any_mut()
        .and_then(|a| a.downcast_mut::<crate::modules::cron::CronModule>())
    { cm.set_scheduler(...); }
});
```

如果模块和引擎的交互都通过 `downcast`，那更换模块实现就无从谈起——因为调用方知道的不是「这是一个提供记忆功能的模块」，而是「这是一个叫 MemoryModule 的具体类型」。

#### 痛点 D：锁重入风险
`CompressModule` 内部有两条路径：
- `do_compress()` — 通过 `EngineHandle.with_module("memory", ...)` 获取记忆数据
- `compress_entries()` — 由调用方（调度器）预取记忆数据后传入

调度器的 `process()` 流程是：
1. 持有 `modules` 的 Mutex → 调 `with_module("memory", ...)` 取数据
2. 释放 memory 的锁
3. 调 `execute_tool_on("compress", "compress_from_entries", ...)` → 持有 compress 的锁

这条路径是对的。但 `do_compress()` 路径（通过 `compress_now` 工具或 `check_and_compress` 内部触发）仍然保持「在 compress 的锁里请求 memory 的锁」的模式，如果调用方同时在 memory 锁里调了 compress，就会死锁。

调度器的 `process()` 里面还有另一个锁问题：压缩调用后面紧跟着 LLM 调用（第 298-317 行），`ToolCallLoop::run` 可能在 LLM 工具调用中再次请求模块锁——但目前没有明确的锁层级约定文档。

> 葵觉得透闪石目前的锁能跑是因为压力不够大。线程一多、调用一密集，锁逆序问题会冒出来。

#### 痛点 E：模块之间不得不用名字互相了解
`required_modules()` 和 `requires()` 用字符串标识能力，这是对的。但模块之间的实际交互却绕过能力系统，通过名字直接 downcast：
- `"memory"` — 调度器知道这个模块的名字
- `"compress"` — 调度器知道这个名字
- `"emotion"` — 调度器知道这个名字
- `"skill"` — 调度器知道这个名字
- `"delegation"` — 引擎知道这个名字
- `"cron"` — 引擎知道这个名字

一旦名字变了或换了一个实现同名模块，所有这些硬编码的名字引用都得改。能力系统和实际访问路径之间没有桥。

#### 痛点 F：跨 crate 复用 vs 单 crate 膨胀
压缩逻辑在 `tremolite-compress` crate 里，目录在 workspace 下，但 `CompressModule` 的 Module 实现又在 `tremolite-core` 的 modules 目录下。这俩 crate 之间是什么关系？

同样模糊的还有：
- `tremolite-cron`（类型定义）vs `tremolite-core/src/modules/cron.rs`（Module 实现）
- `tremolite-plugin`（废弃）vs `tremolite-core/src/module/process_module.rs`（进程模块）
- `tremolite-config` 有自己的 `CronScheduleConfig`、`CronActionConfig`，而 `tremolite-cron` 有 `Schedule`、`CronAction`

谁是谁的基础？谁该依赖谁？没有明确的合约。

---

## 二、重构目标

### 2.1 模块发现 — 配置驱动注册

**现状：** `register_module()` 硬编码在 main.rs。
**目标：** config.toml 中声明要加载哪些模块：

```toml
[modules]
enabled = ["emotion", "memory", "session", "attention", "skill", "compress", "cron", "webhook"]
disabled = ["mcp", "reflection", "delegation"]
```

引擎启动时读取 `[modules]` 配置，只注册 enabled 列表中的模块。允许运行时通过命令/tool 启用禁用模块（可持久化到配置）。
核心模块（emotion、memory、session）有默认启用 fallback——即使配置里没写也启用。

**要求：**
- 模块注册不再是代码变更
- 禁用模块后，依赖它的其他模块在启动时收到「模块缺失」错误
- 能力索引随模块启用/禁用动态更新

### 2.2 能力系统 — 基于 trait 的模块间交互

**现状：** 模块间通过名字 + downcast 直接访问内部数据结构。
**目标：** 定义 4-5 个核心能力 trait，调度器和模块之间只通过这些 trait 交互。

```rust
/// 历史消息提供者
pub trait HistoryProvider: Send + Sync {
    fn recent_messages(&self, session_id: &str, limit: usize) -> Vec<Message>;
    fn store_message(&self, session_id: &str, role: &str, content: &str);
}

/// 上下文压缩器
pub trait ContextCompressor: Send + Sync {
    fn compress(&self, entries: &[MemoryEntry]) -> String;
    fn is_over_threshold(&self, text: &str) -> bool;
}

/// 情绪检测器
pub trait EmotionDetector: Send + Sync {
    fn detect(&self, input: &str) -> EmotionResult;
    fn current_style(&self) -> Option<String>;
    fn composite_emotion(&self) -> String;
}

/// 技能/学习记录器
pub trait SkillRecorder: Send + Sync {
    fn record_tool_use(&self, session_id: &str, tool_name: &str, success: bool);
}

/// 子 session 调度器（供 delegation/cron 模块使用）
pub trait SessionDispatcher: Send + Sync {
    fn dispatch(&self, input: &str, session_id: &str, channel: &str) -> String;
    fn dispatch_async(&self, input: &str, session_id: &str, channel: &str) -> Receiver<String>;
}
```

**要求：**
- 调度器的 `process()` 方法通过 trait 调用功能，不再 downcast
- 运行时可以替换 trait 的实现（换记忆后端、换压缩算法）
- 能力系统在模块注册时自动断言：模块声称提供的能力与其实现的 trait 一致
- 向后兼容：旧模块可以逐步迁移，不一次性要求所有模块都实现 trait

### 2.3 调度器注入 → 调度器发现

**现状：** 引擎在 `create_scheduler()` 中主动把 `inbound_tx` 注入到 `DelegationModule` 和 `CronModule`。
**目标：** 实现 `SessionDispatcher` trait 的模块在运行时注册自己为「可分发 session 任务」的 handler。Cron 模块通过能力发现拿到分派通道，不依赖引擎特殊注入。

**要求：**
- 消除引擎对特定模块的 `with_module_mut("delegation", ...)` 和 `with_module_mut("cron", ...)` 注入路径
- 新增模块如想分发 session 任务，只需实现 `SessionDispatcher` trait 并注册
- 默认实现指向当前调度器的 inbound 通道

### 2.4 锁层级形式化

**现状：** 没有锁层级文档。`do_compress()` 在 CompressModule 锁中请求 MemoryModule 锁是不安全的。
**目标：** 定义全局锁获取顺序：

```
锁层级（获取顺序，从外到内）：
1. ModuleRegistry 的内锁（最外层）
2. 模块的 EngineHandle 操作（中间层）
3. 模块自身锁/内部状态（最内层）

约束：
- 模块在 execute_tool() 或 on_event() 中（处于自身锁内）不得通过 EngineHandle 获取其他模块的锁
- 需要其他模块数据时，数据必须在进入 execute_tool()/on_event() 前预取好
- compress_entries() 模式应作为规范（预取 → 传值 → 处理），do_compress() 模式应被禁止
```

**要求：**
- 移除 `do_compress()` 方法。`CompressModule` 只通过 `compress_entries()` 接收预取数据
- `check_and_compress()` 改为接受外部传入的条目引用，不在内部请求 memory
- 调度器的 process() 流程中，涉及多个模块调用的步骤，数据必须预取后传值
- 文档化锁层级，添加 `// SAFETY:` 注释说明每次锁获取的层级位置
- 考虑引入 `LockHierarchy` 调试断言（debug build 时检查获取顺序是否违反层级）

### 2.5 模块健康与内省

**现状：** 除了 `display_status()` 之外，没有标准的方式来查询模块运行状态。
**目标：** 每个模块暴露结构化健康数据：

```rust
pub struct ModuleHealth {
    pub id: String,
    pub name: String,
    pub version: String,
    pub status: ModuleStatus,        // Running, Degraded, Error, Stopped
    pub uptime_secs: u64,
    pub memory_approx_bytes: u64,
    pub message_count: u64,
    pub error_count: u64,
    pub provides: Vec<Capability>,
    pub requires: Vec<Capability>,
    pub tool_count: usize,
    pub last_error: Option<String>,
    pub details: HashMap<String, String>,
}
```

**要求：**
- Module trait 增加 `fn health(&self) -> ModuleHealth` 默认实现
- 引擎提供 `GET /admin/health` 端点聚合所有模块健康数据
- 调度器在 worker 启动时检查依赖模块的健康状态
- 模块可以报告降级（degraded）状态而不崩溃，如「记忆模块在线但 embedding 服务不可用」

### 2.6 crate 边界清理

**现状：**
- `tremolite-compress`（独立 crate）vs `tremolite-core/src/modules/`（core 内模块实现）
- `tremolite-cron`（独立 crate）vs core 中的 `CronModule`
- `tremolite-plugin`（废弃）vs `process_module.rs`
- config 有自己的 cron 类型定义，与 tremolite-cron 重复

**目标：**
- **模块实现统一放在定义该模块的 crate 中**。如果 `tremolite-compress` 是模块 crate，那它的 `CompressModule`（Module 实现）应该在那里，而不是在 core 里。
  - 或者反过来：core 的 `modules/` 目录是唯一模块实现场所，移除独立模块 crate。
  - 选一个方向，不要模糊边界。
- `tremolite-cron` 要么被 core 吸收（类型定义和 Module 实现放一起），要么成为真正的模块 crate（把 `CronModule` 移过去）。
- 移除 `tremolite-plugin` crate。它的最后引用被清理后，从 workspace 中删除。
- Config crate 中的 `CronScheduleConfig`/`CronActionConfig` 要么使用 `tremolite-cron` 的类型，要么吸收并删除 `tremolite-cron`。

### 2.7 按功能维度重构调度器 process() 方法

**现状：** `SessionWorker::process()`（scheduler.rs 第 225-333 行）是一个 108 行的巨型方法，混合了：
- prompt 构建
- 工具列表获取
- 历史消息获取（downcast memory）
- 压缩数据获取（downcast compress）
- LLM 调用
- 工具调用记录（downcast skill）
- 情绪 fallback

**目标：** 拆分为清晰的阶段：

```rust
fn process(&mut self, input: &str, channel: &str) -> String {
    // Phase 1: 数据收集（通过 trait，不 downcast）
    let context = self.collect_context(input);
    
    // Phase 2: prompt 构建
    let prompt = self.build_prompt(&context);
    
    // Phase 3: LLM 调用（含工具循环）
    let result = self.invoke_llm(&prompt);
    
    // Phase 4: 记录与持久化（通过 trait，不 downcast）
    self.record_result(&result);
    
    result.content
}
```

**要求：**
- `collect_context()` 通过 trait 获取：历史消息（HistoryProvider）、情绪状态（EmotionDetector）、压缩上下文（ContextCompressor）、模块 prompt 段
- `record_result()` 通过 trait 记录：技能练习（SkillRecorder）、记忆存储（HistoryProvider）
- 拆分后每个阶段不超过 30 行，职责单一，可测试
- 旧调度器在 migration 完成前作为 deprecated 路径保留

### 2.8 模块配置解耦

**现状：** 模块的配置分散在各处：
- `EmotionModule` 的 `with_tone_map(tm_path, em_path)` 在 main.rs 中硬编码路径
- `MemoryModule` 接收 `data_dir` 路径
- `McpModule` 接收配置列表
- 每个模块有自己的构造函数参数签名

**目标：** 模块从配置系统获取自己的配置段：

```toml
[modules.emotion]
tone_map = "./data/tremolite/tone_map.json"
emotion_file = "./data/tremolite/emotion.json"

[modules.memory]
backend = "sqlite"
path = "./data/tremolite/memory.db"

[modules.compress]
auto = true
threshold_tokens = 64000
blocks = 5
ratios = ["Delete", "20%", "40%", "60%", "Full"]

[modules.cron]
enabled = true

[modules.reflection]
interval_messages = 10
```

**要求：**
- 引擎初始化时，为每个模块传递其 `[modules.<id>]` 配置段（如果存在）
- Module trait 增加 `fn configure(&mut self, config: &HashMap<String, serde_json::Value>) -> Result<(), ModuleError>` 默认方法
- 模块的 `configure()` 实现解析自己的配置段，与构造函数分离
- Config crate 在 `[modules]` 下不做类型校验——每个模块自己解析，保持 config 轻量

### 2.9 运行时热加载（远期目标）

**要求：**
- 通过命令/API 在运行时注册新的模块（动态链接或进程模块）
- 通过命令/API 在运行时卸载/禁用模块（优雅关闭）
- 模块版本兼容性检查（API 版本声明）
- 热加载期间不影响正在进行的会话

> 不建议在 Phase 1 实现。先完成 2.1-2.8 再考虑热加载。标在这里是为了让架构不把这条路堵死。

---

## 三、实施顺序

### Phase A：立即动手的

| 序号 | 工作 | 涉及文件 | 预估复杂度 |
|------|------|---------|-----------|
| A1 | config 引入 `[modules]` 段，引擎按配置注册模块 | config crate, main.rs, `TremoliteEngine` | 低 |
| A2 | `required_modules()` 依赖检查在 `register()` 中实施 | `ModuleRegistry` | 低 |
| A3 | 锁层级形式化 + 移除 `do_compress()` | `CompressModule`, scheduler | 中 |
| A4 | 定义 `HistoryProvider`、`EmotionDetector` trait | `tremolite-core` 新文件 | 中 |

### Phase B：逐步重构的

| 序号 | 工作 | 涉及文件 | 预估复杂度 |
|------|------|---------|-----------|
| B1 | 调度器 process() 按 trait 重构 | `scheduler.rs` | 高 |
| B2 | 调度器注入路径替换为 `SessionDispatcher` trait | `engine.rs`, `CronModule`, `DelegationModule` | 中 |
| B3 | 模块健康系统 | Module trait, HTTP admin 端点 | 中 |
| B4 | 模块配置解耦 | Module trait, 各模块 | 中 |

### Phase C：边界清理的

| 序号 | 工作 | 涉及文件 | 预估复杂度 |
|------|------|---------|-----------|
| C1 | crate 边界决策 + 执行 | workspace, `tremolite-compress`, `tremolite-cron`, `tremolite-plugin` | 中 |
| C2 | 废弃代码清理 | `tremolite-plugin`, process_module.rs 重复 | 低 |

---

## 四、不做的事情

- **不换语言**。透闪石是 Rust，继续是 Rust。
- **不改 Module trait 的现有方法签名**。添加新方法（`configure()`、`health()`），不改已有签名。现有模块不需要因重构而修改代码。
- **不会为「统一」而统一**。如果一个模块只有 50 行且没有其他模块依赖它（比如 tools.rs），不需要为了「它也要是模块」而把它拆成 crate。重量级的 crate 边界只在有明确解耦价值时引入。
- **不在 Phase A 做热加载**。先把冷启动做对再做热的。

---

## 五、验收标准

1. **配置驱动注册**：移除 `main.rs` 第 191-218 行中的任意几个 `register_module()` 调用，改在 config.toml 的 `disabled` 列表中关闭对应模块，重启后该模块不再加载。依赖它的模块在启动时报可理解的错误。

2. **依赖检查**：如果在 config.toml 中启用了 compress 但禁用了 memory，启动时报错：
   ```
   Module 'compress' requires 'memory' which is not registered.
   ```

3. **无 downcast 调度器**：调度器 `process()` 方法不再调用 `m.as_any().and_then(|a| a.downcast_ref::<...>())`。所有模块交互通过 trait 完成。

4. **锁安全**：`compress_entries()` 是 CompressModule 唯一的数据入口。`do_compress()` 不存被移除。调度器的 process() 数据流是：预取 → 传值 → 处理。

5. **模块健康**：`GET /admin/health` 返回所有模块的健康状态 JSON，包括 up/down、错误计数、工具数量。

6. **配置解耦**：EmotionModule 的 tone_map 路径不硬编码在 main.rs 中，从 `[modules.emotion]` 配置段读取。

7. **架构文档与代码一致**：本文档的需求说明书反映在 PLAN.md、README 和实际代码中。不产生「文档写了但代码没做」或「代码做了但文档没更新」的差距。
