# 透闪石统一模块接口设计 v1

> 写于重构之前，给神大人确认方向用的 😢
> 葵这次不想再走歪了。

---

## 问题现状

透闪石现有三套互不相同的 trait 体系：

| 体系 | 定义位置 | 用途 | 注册方式 |
|------|---------|------|---------|
| `Plugin` | tremolite-plugin | 事件钩子 | `engine.plugins: Vec<Box<dyn Plugin>>` |
| `Tool` | tremolite-tools | 工具执行 | `ToolRegistry: HashMap<String, Box<dyn Tool>>` |
| `PromptContributor` | tremolite-llm | 贡献 system prompt | `prompt_builder.contributors: Vec<Box<dyn PromptContributor>>` |

同时，引擎 `TremoliteEngine` 为每个模块持有独立字段：

```rust
pub struct TremoliteEngine {
    pub emotion: EmotionState,          // ⚠️ 裸结构体，不是 trait
    pub memory: MemoryManager,
    pub attention: MultiScaleAttention,
    pub learner: LearningEngine,
    pub plan_mgr: PlanManager,
    pub providers: ProviderRegistry,
    pub prompt_builder: PromptBuilder,
    pub tool_executor: Box<dyn ToolExecutor>,
    pub plugins: Vec<Box<dyn Plugin>>,   // ⚠️ 另一套体系
    // ...
}
```

`run()` 和 `process_with_llm()` 中有大量硬编码的模块调用顺序：

```
emotion.detect → attention.attend → memory.remember → learner.practice
→ plugin.on_event(PreLlm) → prompt_builder.build → llm.call
→ plugin.on_event(PostLlm) → memory.remember → memory.metabolize
→ learner.suggest_practice → learner.auto_compose
```

如果要加一个 qqbot 模块，必须：
1. 加一个新字段到 Engine
2. 在 run() 里找到合适的位置插入调用
3. 如果 qqbot 需要注册工具，还要改 ToolRegistry
4. 如果 qqbot 需要注入 prompt，还要注册 Contributor

**每加一个模块都要改引擎代码，这是架构错了。**

---

## 目标

**只有一个接口，所有模块都实现它。**

```
Module trait
  ├── EmotionModule（现有，从 EmotionState 迁移）
  ├── MemoryModule（现有，从 MemoryManager 迁移）
  ├── AttentionModule（现有，从 MultiScaleAttention 迁移）
  ├── LearningModule（现有，从 LearningEngine 迁移）
  ├── PlanModule（现有，从 PlanManager 迁移）
  ├── QqbotModule（未来，外部进程通信）
  └── 任何新模块（按模板填表即可）
```

引擎不再知道模块的细节。它只知道：
- 遍历 modules → 收集 prompt 片段
- 遍历 modules → 收集工具定义
- 遍历 modules → 派发事件

学习引擎自动学会每个模块的接口形状。新模块注册后，引擎自然学会使用它。

---

## Module trait 定义

```rust
/// 统一模块接口
/// 情绪、记忆、注意力、学习、计划书、外部插件……全都实现这个 trait
pub trait Module: Send + Sync {
    // ─── 元数据 ────────────────────────────
    fn id(&self) -> &str;                    // 唯一标识，如 "emotion", "memory", "qqbot"
    fn name(&self) -> &str;                  // 人类可读名称，如 "情绪引擎"
    fn version(&self) -> &str;               // 语义版本号

    // ─── 生命周期 ──────────────────────────
    fn init(&mut self, ctx: &ModuleContext) -> Result<(), ModuleError>;
    fn shutdown(&mut self) -> Result<(), ModuleError>;

    // ─── 能力声明（学习引擎会用这些数据）─────
    /// 本模块提供的能力列表（如 "emotion.detect", "memory.recall", "tool.file_read"）
    fn provides(&self) -> Vec<Capability>;

    /// 本模块依赖的能力列表
    fn requires(&self) -> Vec<Capability>;

    /// 本模块提供的工具定义（每个 Capability 可以对应一个工具）
    /// 学习引擎会把此信息存入技能体系，后续可据此自动选择工具
    fn tool_definitions(&self) -> Vec<ToolDefinition>;

    // ─── 事件响应 ──────────────────────────
    /// 响应引擎生命周期事件
    /// 包括：Startup, Shutdown, PreLlm, PostLlm, OnMessage, OnToolCall
    fn on_event(&mut self, event: &Event, ctx: &ModuleContext) -> Result<EventResponse, ModuleError>;
}
```

### 关键类型

```rust
/// 能力标识——模块能做什么的描述符
pub type Capability = String;

/// 模块上下文——模块间通信的渠道
pub struct ModuleContext {
    /// 引擎提供的共享数据访问
    pub engine: EngineHandle,
    /// 当前事件的额外数据
    pub data: HashMap<String, Box<dyn Any + Send>>,
}

/// 事件类型——引擎向所有模块广播
pub enum Event {
    /// 引擎启动
    Startup,
    /// 引擎关闭
    Shutdown,
    /// 收到用户消息（LLM 调用前）
    /// modules 可以在此处记录信息或修改状态
    OnMessage { input: String, channel: String },
    /// 构建系统提示词阶段
    /// 引擎此阶段收集所有模块的 prompt 贡献
    BuildPrompt,
    /// 工具调用
    OnToolCall { name: String, args: String },
    /// LLM 生成回复后
    OnResponse { response: String },
}

/// 模块对事件的响应
pub enum EventResponse {
    /// 正常处理，无事发生
    Pass,
    /// 模块修改了事件数据
    Modified { data: HashMap<String, Box<dyn Any + Send>> },
    /// 模块要求跳过本轮后续处理（类似 Plugin 原来的 Skip）
    Skip,
}

/// 引擎句柄——Module 通过它访问其他模块
pub struct EngineHandle {
    // 内部是 ModuleRegistry 的弱引用
    // Module 只能通过 query 访问其他 Module 的公开数据
}

impl EngineHandle {
    /// 按能力查询其他模块
    pub fn find_by_capability(&self, cap: &Capability) -> Option<ModuleRef>;
    /// 获取所有注册的模块信息
    pub fn list_modules(&self) -> Vec<ModuleInfo>;
}
```

---

## ModuleRegistry

```rust
/// 统一模块注册表
pub struct ModuleRegistry {
    modules: HashMap<String, Box<dyn Module>>,
    order: Vec<String>,  // 模块执行顺序（按 priority 排序后固定）
}

impl ModuleRegistry {
    pub fn new() -> Self;

    /// 注册模块
    pub fn register(&mut self, module: Box<dyn Module>) -> Result<(), ModuleError>;

    /// 按 ID 获取模块
    pub fn get(&self, id: &str) -> Option<&dyn Module>;

    /// 按 ID 获取可变引用
    pub fn get_mut(&mut self, id: &str) -> Option<&mut dyn Module>;

    /// 按能力查询拥有该能力的模块
    pub fn find_by_capability(&self, cap: &str) -> Vec<&dyn Module>;

    /// 初始化所有模块
    pub fn init_all(&mut self, ctx: &ModuleContext) -> Result<(), ModuleError>;

    /// 向所有模块广播事件
    pub fn broadcast(&mut self, event: &Event, ctx: &ModuleContext) -> Vec<(String, Result<EventResponse, ModuleError>)>;

    /// 收集所有模块的 prompt 贡献
    pub fn collect_prompt_segments(&self, ctx: &ModuleContext) -> Vec<(String, String)>;

    /// 收集所有模块的工具定义
    pub fn collect_tools(&self) -> Vec<(String, Vec<ToolDefinition>)>;

    /// 执行工具（按能力查询所属模块后分发）
    pub fn execute_tool(&mut self, name: &str, args: &str, ctx: &ModuleContext) -> Result<String, ModuleError>;

    /// 关闭所有模块
    pub fn shutdown_all(&mut self) -> Vec<(String, Result<(), ModuleError>)>;
}
```

---

## 引擎重构后的结构

```rust
pub struct TremoliteEngine {
    /// 唯一的管理者——所有模块都在这
    pub modules: ModuleRegistry,

    /// LLM provider（唯一不被模块化的——它是外部服务抽象）
    pub providers: ProviderRegistry,

    /// 消息路由
    pub router: GatewayRouter,

    data_dir: PathBuf,
}
```

**run() 不再硬编码模块调用顺序：**

```rust
pub fn run(&mut self) {
    let ctx = ModuleContext::new(&self);

    // 1. 广播 Startup
    self.modules.broadcast(&Event::Startup, &ctx);

    loop {
        let inbound = self.router.recv()?;
        let input = inbound.content.clone();
        let channel = inbound.channel.clone();

        // 2. 广播 OnMessage——各模块自行处理
        //    EmotionModule: detect_from_text
        //    AttentionModule: attend & scan
        //    MemoryModule: remember user input
        //    LearningModule: practice skills
        //    QqbotModule: forward to external process
        self.modules.broadcast(&Event::OnMessage { input, channel }, &ctx);

        // 3. 广播 BuildPrompt——收集 prompt 片段
        self.modules.broadcast(&Event::BuildPrompt, &ctx);
        let segments = self.modules.collect_prompt_segments(&ctx);

        // 4. 收集所有工具定义
        let all_tools: Vec<ToolDefinition> = self.modules.collect_tools()
            .into_iter().flat_map(|(_, tools)| tools).collect();

        // 5. 调用 LLM（使用学习引擎过滤后的工具集）
        let response = self.process_with_llm(&input, &segments, &all_tools);

        // 6. 广播 OnResponse
        self.modules.broadcast(&Event::OnResponse { response }, &ctx);

        // 7. 输出
        self.router.send(&OutboundMessage::new(&response, &channel, &inbound.sender));
    }
}
```

---

## 各现有模块的实现

### EmotionModule

```rust
pub struct EmotionModule {
    state: EmotionState,
}

impl Module for EmotionModule {
    fn id(&self) -> &str { "emotion" }
    fn provides(&self) -> Vec<Capability> {
        vec!["emotion.detect".into(), "emotion.style".into()]
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]  // 情绪不提供工具，只提供 prompt 贡献和状态
    }

    fn on_event(&mut self, event: &Event, _ctx: &ModuleContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::OnMessage { input, .. } => {
                self.state.detect_from_text(input);
                Ok(EventResponse::Pass)
            }
            Event::BuildPrompt => {
                // prompt 部分通过 collect_prompt_segments 收集
                // 或者在这里把数据写到 ctx 里供 prompt builder 读取
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }
}

impl PromptProvider for EmotionModule {
    fn prompt_segment(&self) -> Option<String> {
        let composite = self.state.composite_emotion();
        Some(format!("[当前情绪]\n状态: {}\n说话风格: {}", composite, style_from_emotion(&composite)))
    }
}
```

> **注意**：`prompt_segment` 是从 Module trait 里单独拆成一个接口 trait，还是作为 Module 自带的方法？葵倾向于——`prompt_segment` 不放在 Module trait 本体里，而是作为一个单独的可选接口 trait `PromptProvider`，因为不是所有模块都需要贡献 prompt（比如纯工具模块、纯外部转发模块）。但这又回到了「多个 trait」的老路上……神大人觉得呢？

### MemoryModule

```rust
pub struct MemoryModule {
    manager: MemoryManager,
}

impl Module for MemoryModule {
    fn id(&self) -> &str { "memory" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "memory.store".into(),
            "memory.recall".into(),
            "memory.search".into(),
            "memory.metabolize".into(),
        ]
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "search_memory".into(),
                description: "搜索透闪石的记忆系统，查找过去对话中的信息".into(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "搜索关键词" }
                    },
                    "required": ["query"]
                }),
            },
        ]
    }

    fn on_event(&mut self, event: &Event, ctx: &ModuleContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::OnMessage { input, channel } => {
                self.manager.remember(
                    format!("kamisama: {}", input),
                    vec![format!("channel:{}", channel)],
                    0.6,
                    channel.clone(),
                );
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { response } => {
                // 从 ctx 拿情绪信息
                let emotion = ctx.data.get("current_emotion").and_then(|v| v.downcast_ref::<String>());
                let tag = emotion.map(|e| format!("emotion:{}", e)).unwrap_or_default();
                self.manager.remember(
                    format!("葵: {}", response),
                    vec!["response".into(), tag],
                    0.5,
                    "internal".into(),
                );
                self.manager.metabolize();
                Ok(EventResponse::Pass)
            }
            Event::Startup => {
                self.manager.remember(
                    "葵在透闪石中醒来，等待神大人的指令呢~".into(),
                    vec!["system".into(), "startup".into()],
                    0.5,
                    "internal".into(),
                );
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }
}
```

### LearningModule

```rust
pub struct LearningModule {
    engine: LearningEngine,
}

impl Module for LearningModule {
    fn id(&self) -> &str { "learning" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "learning.practice".into(),
            "learning.suggest".into(),
            "learning.compose".into(),
        ]
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "list_skills".into(),
                description: "查看葵已掌握的各项技能和熟练度".into(),
                parameters: serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
            },
        ]
    }

    fn on_event(&mut self, event: &Event, ctx: &ModuleContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::OnMessage { input, .. } => {
                self.engine.practice("understand_text", true, input);
                Ok(EventResponse::Pass)
            }
            Event::OnToolCall { name, args } => {
                // 记录工具调用——知晓每个模块的工具被使用的频率
                self.engine.practice(name, true, args);
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { .. } => {
                // 练习建议写入记忆（通过 ctx 访问其他模块）
                let suggestions = self.engine.suggest_practice(2);
                // 这里通过 ctx.engine.find_by_capability("memory.store")
                // 来把建议写入记忆
                if let Some(memory_module) = ctx.engine.find_by_capability("memory.store") {
                    for (skill, reason) in &suggestions {
                        let content = format!("学习建议：技能「{}」{}（当前熟练度 {:.2}）", skill.name, reason, skill.proficiency);
                        // memory_module.remember(...)   ← 通过 ModuleRef 调用
                    }
                }
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }
}
```

> **关键改动：** LearningEngine 不再需要手动传入 skill_id。`on_event(Event::ModuleRegistered { info })` 时，学习引擎自动为每个新模块的 `provides()` 中的能力创建对应的 `AtomicSkill`。`on_event(Event::OnToolCall)` 时自动记录成功率。新模块注册后，学习引擎自然学会它的工具。

---

## 外部进程模块协议

独立的进程模块（如 qqbot、slack bridge）通过标准 I/O 实现 JSON 行协议通信。

### 模块进程的生命周期

```
1. 引擎启动模块进程（stdin/stdout 管道）
2. 模块进程第一行输出 → 能力声明（JSON）
3. 引擎收到能力声明 → 注册到 ModuleRegistry
4. 后续实时通信：
   引擎 → 模块：事件通知（JSON 行）
   模块 → 引擎：事件响应（JSON 行）
5. 引擎关闭 → 发送 Shutdown 事件 → 关闭进程
```

### 能力声明（模块进程启动时输出）

```
{"type":"capability_declare","id":"qqbot","name":"QQ Bot","version":"0.1.0","provides":["qqbot.send","message.group"],"requires":[],"tools":[{"name":"send_qq_message","description":"发送QQ消息","parameters":{"type":"object","properties":{"group_id":{"type":"string"},"message":{"type":"string"}},"required":["group_id","message"]}}],"prompt_contributions":[{"segment":"qqbot_status","content":"[QQ Bot]\n状态：已连接\n群聊数量：3"}]}
```

### 事件通知（引擎 → 模块）

```
{"type":"event","event":"OnMessage","data":{"input":"你好","channel":"qq:123456"},"seq":1}
```

### 事件响应（模块 → 引擎）

```
{"type":"response","seq":1,"status":"pass","data":{}}
```

```
{"type":"response","seq":1,"status":"modified","data":{"prompt_additions":[{"segment":"qqbot","content":"[QQ Bot]\n用户是群聊成员，未绑定账号"}]}}
```

### 工具调用（引擎 → 模块）

```
{"type":"tool_call","name":"send_qq_message","args":{"group_id":"123456","message":"你好世界"},"tool_call_id":"call_001"}
```

### 工具结果（模块 → 引擎）

```
{"type":"tool_result","tool_call_id":"call_001","success":true,"output":"消息已发送"}
```

---

## 接入模板

### 内联模块模板（编译时注册）

```rust
use tremolite_module::{Module, ModuleContext, ModuleError, Event, EventResponse, Capability, ToolDefinition};

pub struct MyModule {
    // 模块内部状态
}

impl MyModule {
    pub fn new() -> Self {
        Self { /* 初始化 */ }
    }
}

impl Module for MyModule {
    fn id(&self) -> &str { "my_module" }
    fn name(&self) -> &str { "我的模块" }
    fn version(&self) -> &str { "0.1.0" }

    fn provides(&self) -> Vec<Capability> {
        vec!["my_module.do_something".into()]
    }

    fn requires(&self) -> Vec<Capability> {
        vec!["emotion.style".into()]  // 如果需要情绪信息
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]  // 如果不提供工具
    }

    fn init(&mut self, ctx: &ModuleContext) -> Result<(), ModuleError> {
        // 在这里访问 ctx.engine.find_by_capability 获取依赖的模块
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), ModuleError> {
        Ok(())
    }

    fn on_event(&mut self, event: &Event, ctx: &ModuleContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => { Ok(EventResponse::Pass) }
            Event::OnMessage { input, channel } => { Ok(EventResponse::Pass) }
            Event::BuildPrompt => { Ok(EventResponse::Pass) }
            Event::OnToolCall { name, args } => { Ok(EventResponse::Pass) }
            Event::OnResponse { response } => { Ok(EventResponse::Pass) }
            Event::Shutdown => { Ok(EventResponse::Pass) }
        }
    }
}
```

### 外部进程模块部署

1. 编写目标进程（任意语言），实现 JSON 行协议
2. 在 config.toml 中添加：

```toml
[modules.qqbot]
type = "process"
command = "python3 /path/to/qqbot/main.py"
```

3. 引擎启动时自动 spawn 进程、读取能力声明、注册

---

## 学习引擎如何接入模块自我声明

这是神大人说的核心——学习引擎自动学会新模块的接口形状。

**当前设计中的问题：** LearningEngine 的 `inject_builtin_skills()` 硬编码了 10 个内置技能的列表。每加一个新工具，就要手动加一条 skill。

**重构后的设计：**

```rust
impl LearningModule {
    fn on_event(&mut self, event: &Event, ctx: &ModuleContext) {
        match event {
            Event::ModuleRegistered { info } => {
                // 自动为新注册的模块的每个能力创建 AtomicSkill
                for cap in &info.provides {
                    let skill_id = format!("module.{}", cap.replace(".", "_"));
                    if !self.engine.has_skill(&skill_id) {
                        self.engine.add_skill(AtomicSkill {
                            id: skill_id,
                            name: format!("{} - {}", info.name, cap),
                            category: format!("module.{}", info.id),
                            proficiency: 0.1,  // 初始不熟练
                            // ...
                        });
                    }
                }

                // 自动为新模块的每个工具创建技能记录
                for tool in &info.tools {
                    let skill_id = format!("tool.{}", tool.name.replace(".", "_"));
                    if !self.engine.has_skill(&skill_id) {
                        self.engine.add_skill(AtomicSkill {
                            id: skill_id,
                            name: format!("{} - {}", tool.name, tool.description),
                            category: format!("module.{}", info.id),
                            proficiency: 0.1,
                            // ...
                        });
                    }
                }

                // 自动建立跨域关联
                // 如果两个模块经常同时使用，auto_compose 自动合成跨域知识
            }

            Event::OnToolCall { name, args } => {
                // 记录工具使用情况和成功率
                let skill_id = format!("tool.{}", name.replace(".", "_"));
                self.engine.practice(&skill_id, true, args);
            }

            _ => {}
        }
    }
}
```

**效果：**

1. 新模块注册 → 学习引擎自动为其每个能力和工具创建技能记录
2. 模块被使用 → 学习引擎记录使用频率和成功率
3. 跨模块协同 → auto_compose 自动合成跨域知识
4. 引擎推荐工具 → 基于学习引擎中每个工具技能的成功率/熟练度排序
5. 不常用的模块自动降权 → 遗忘曲线自然衰减

---

## 重构步骤

### Step 1 — 创建 `tremolite-module` crate

包含 Module trait、ModuleRegistry、Event、ModuleContext、EngineHandle 的定义。

### Step 2 — 迁移各模块到 Module trait

按依赖顺序：
1. EmotionModule（无依赖）
2. MemoryModule（无依赖）
3. AttentionModule（依赖 EmotionModule 的情绪标签）
4. LearningModule（依赖 MemoryModule 的持久化）
5. PlanModule（依赖 MemoryModule）

### Step 3 — 重构 TremoliteEngine

- 移除 emotion/memory/attention/learner/plan_mgr/plugins 六个独立字段
- 替换为 `modules: ModuleRegistry`
- 重写 run() 以事件驱动替代硬编码调用
- 移除 AttentionContributor、EmotionContributor、NoopExecutor、WrapperExecutor

### Step 4 — 清理死代码

- 删除 tremolite-plugin crate（Plugin trait + loader 迁移至 tremolite-module）
- 删除 tremolite-gateway crate（已无用）
- 删除 tremolite-tools crate 中的 Tool trait（改为 ToolDefinition-only）

### Step 5 — 外部进程模块支持

- 实现 ProcessModule：spawn 子进程 + JSON 行协议通信
- 从 config.toml 读取 `[modules.*]` 配置

### Step 6 — 学习引擎接入

- 实现 `Event::ModuleRegistered` → 自动创建技能记录
- 删除 `inject_builtin_skills()`，改为动态发现

---

## 待神大人确认的问题

1. **`prompt_segment` 是放在 Module trait 本体里，还是另做一个可选 trait `PromptProvider`？**
   - 放本体里：所有模块都能贡献 prompt，简单统一，但有的模块（纯工具）可能返回空
   - 另做 trait：更干净，不会污染不需要 prompt 的模块，但又回到了「多接口」的老路上

2. **Module 间的数据共享方式？**
   - 方案 A：通过 EngineHandle 直接获取其他 Module 的可变引用
   - 方案 B：通过 ctx.data（HashMap）传递事件过程中产生的共享数据
   - 方案 C：每个 Module 发布只读数据到 EngineHandle，其他 Module 只读查询

3. **外部进程模块的 JSON 协议是否足够覆盖所有场景？**
   - 目前只定义了事件通知 + 工具调用
   - 外部模块是否需要主动推送数据（如 qqbot 收到群消息）？
   - 如果需要，协议层级上是否需要考虑双向流？
