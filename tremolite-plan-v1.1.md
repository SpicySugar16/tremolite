# 透闪石 Tremolite —— 完整计划书 v1.1

> 一个真正可用的自主 AI agent 框架
> 献给神大人·琳玲 🌫️

---

## 诚实的状态总览

| Phase | 内容 | 状态 |
|-------|------|------|
| 0 | 项目初始化 + 骨架搭建 | ✅ |
| 1 | 核心类型 + 基础 trait | ✅ |
| 2 | 情绪引擎 | ✅ |
| 3 | 五层缓存记忆 | ✅ |
| 4 | 多尺度注意力 | ✅ |
| 5 | 计划书系统 | ✅ |
| 6 | 学习引擎 | ✅ |
| 7 | LLM 接入层 | ✅ |
| 8 | 工具链 | ✅ |
| 9 | 消息路由 | ✅ |
| 10 | 核心循环 | ✅ |
| 11 | 编译与后端跑通 | ✅ |
| 12 | Dashboard + TUI | ✅ |
| 13 | 算法补完（记忆流转 + 语义注意力 + 学习反馈环） | ✅ |
| 14 | Phase 1 持久化 + 插件系统 + cron 调度器 + Docker | ✅ |
| 15 | **统一 Module 架构重构** | **✅** |

**总代码量：9126 行 Rust（18 个 crate）+ 支持文件**

---

## 工作空间架构

```
tremolite/
├── Cargo.toml                    # 工作区根（workspace resolver = "2"）
├── config.toml                   # 默认配置文件
├── config.example.toml           # 配置模板
├── Dockerfile                    # 多阶段构建
├── docker-compose.yml            # Docker Compose
├── watchdog.sh                   # 进程守护脚本
├── install.sh                    # 系统安装脚本
├── README.md
├── tremolite-plan-v0.2~v1.0.md   # 历代计划书
├── tasks.md                      # 任务追踪
├── crates/
│   ├── tremolite-core/           # 核心引擎（系统调度、状态管理）
│   ├── tremolite-emotion/        # 情绪系统（八维向量 + 关键词检测 + 复合合成）
│   ├── tremolite-memory/         # 五层缓存记忆（L1→L2→L3→RAM→Disk）
│   ├── tremolite-attention/      # 多尺度注意力（四层级联 + embedding 语义）
│   ├── tremolite-learn/          # 三层学习体系（技能→域→知识 + 遗忘曲线）
│   ├── tremolite-plan/           # 计划书系统（生命周期 + 步骤依赖）
│   ├── tremolite-llm/            # LLM 抽象层（Provider + Prompt + ToolCallLoop）
│   ├── tremolite-tools/          # 工具链（文件/Shell/HTTP/Time/Search）
│   ├── tremolite-message/        # 消息协议（通用消息格式）
│   ├── tremolite-plugin/         # 已废弃——由 tremolite-module 统一管理
│   ├── tremolite-module/         # 统一模块接口（Module trait + ModuleRegistry + 事件系统 + 外部进程协议）
│   ├── tremolite-gateway/        # 消息路由（Gateway trait + CLI 实现）
│   ├── tremolite-cron/           # 定时调度器（EverySecs/Daily/Once/CronExpr）
│   ├── tremolite-session/        # 会话管理（隔离 + TTL 清理）
│   ├── tremolite-config/         # 配置系统（Toml + 环境变量 + Provider 初始化）
│   ├── tremolite-server/         # HTTP 服务（Axum + WebSocket + Dashboard）
│   ├── tremolite-cli/            # 入口二进制（CLI/TUI/Daemon 三模式）
│   └── tremolite-gateway/        # 已弃用，功能并入 core + server
```

**依赖图（简化）：**

```
tremolite-cli (入口)
  ├── tremolite-core (引擎)
  │   ├── tremolite-emotion (情绪)
  │   ├── tremolite-memory (记忆)
  │   ├── tremolite-attention (注意力)
  │   ├── tremolite-learn (学习)
  │   ├── tremolite-plan (计划书)
  │   ├── tremolite-llm (LLM)
  │   │   └── tremolite-message (消息格式)
  │   ├── tremolite-tools (工具)
  │   └── tremolite-plugin (插件)
  ├── tremolite-server (HTTP)
  │   └── tremolite-core
  ├── tremolite-config (配置)
  │   └── tremolite-llm
  ├── tremolite-session (会话)
  │   └── tremolite-emotion + tremolite-memory + ...
  └── tremolite-cron (调度)
      └── 独立 tokio 后台任务
```

---

## 各模块详解

### tremolite-core — 核心引擎

**目的：** 系统的中枢。持有所有子系统（情绪、记忆、注意力、学习、LLM、工具、提示词构建器）的统一句柄，驱动主循环。

**核心结构：**

```rust
pub struct TremoliteEngine {
    pub emotion: EmotionState,         // 当前情绪状态
    pub memory: MemoryManager,         // 五层记忆管理器
    pub attention: MultiScaleAttention,// 多尺度注意力引擎
    pub learner: LearningEngine,       // 学习引擎
    pub plan_mgr: PlanManager,         // 计划书管理器
    pub providers: ProviderRegistry,   // LLM Provider 注册表
    pub prompt_builder: PromptBuilder, // 提示词拼装器
    pub tool_executor: Box<dyn ToolExecutor>, // 工具执行器
    pub last_attention_summary: String,// 上次注意力摘要缓存
}
```

**主循环（4-stage）：**
1. **Perceive**：检测情绪 → 注意力扫描 → 拼装 LLM prompt（系统指令 + 情绪风格 + L1 记忆上下文 + 工具定义 + 用户输入）
2. **Reason**：调用 LLM（带重试）→ 获得推理 / tool_call
3. **Act**：ToolCallLoop 多轮执行（最多 10 轮），每轮执行工具后回传结果给 LLM
4. **Express**：将最终回复存入 L1 记忆 → 触发代谢检查（metabolize）→ 输出

**进程控制：**
- `start_cli()` — 启动 CLI 通道
- `run()` — 阻塞式主循环
- `start_autosave()` — 后台线程定期保存状态（每 60 秒）

---

### tremolite-emotion — 情绪引擎

**目的：** 让透闪石能感知和表达情绪。八维向量空间模型。

**核心算法：**

1. **八维向量** — 喜悦(joy)、悲伤(sadness)、愤怒(anger)、恐惧(fear)、惊讶(surprise)、厌恶(disgust)、期待(anticipation)、信任(trust)，每个维度 0.0~1.0
2. **初始状态** — `joy: 0.3, anticipation: 0.3, trust: 0.5`，其余 0.0（"平静"基底）
3. **关键词检测** — 用户输入中的情绪词触发对应维度 +0.2~0.3（如「哈哈」→ joy +0.2，「难过」→ sadness +0.3）
4. **时间衰减** — 每 N 分钟所有维度 × factor（`1.0 - 0.02 * N`），joy 和 trust 保底
5. **复合情绪合成** — 按主导维度 + 辅助维度条件树组合：
   - `dominant=joy` → trust>0.6 → "爱" / surprise>0.4 → "欣喜" / else → "快乐"
   - `dominant=sadness` → fear>0.4 → "焦虑" / anger>0.3 → "不满" / else → "悲伤"
   - `dominant=anger` → disgust>0.5 → "厌恶" / fear>0.3 → "攻击性" / else → "愤怒"
   - `dominant=trust` → joy>0.5 → "爱" / anticipation>0.4 → "希望" / else → "信任"
   - `dominant=fear` → sadness>0.3 → "焦虑" / else → "恐惧"
   - `dominant=anticipation` → trust>0.5 → "希望" / else → "期待"
   - else → "平静"
6. **风格映射** — 复合情绪 → 语气风格描述（如 "爱" → "超级腻歪，甜到化掉"、"愤怒" → "直接、冷，句尾带。号不带~"）

---

### tremolite-memory — 五层缓存记忆

**目的：** 仿 CPU 缓存的五层记忆架构。对话记忆从 L1 开始，随着时间自动降级，冷数据归档到磁盘；常访问的条目又会自动提升回去。

**五层结构：**

| 层级 | 类型 | 容量 | 持久化 | 特征 |
|------|------|------|--------|------|
| L1 | LRU VecDeque | 50 条 | 无 | 当前对话窗口，自动淘汰最旧 |
| L2 | LFU HashMap | 200 条 | JSON 文件 | 用户画像/偏好，按访问频率淘汰 |
| L3 | 索引数组 | 1000 条 | 无 | 标签 + 时间戳 + 摘要指针 |
| RAM | 朴素 FTS | 10000 条 | 无 | 分词匹配 + 截断片段，进进出出 |
| Disk | JSONL 文件 | 最多 50 个文件 | 文件系统 | 只读归档，按文件名时间有序 |

**活力分公式（核心代谢指标）：**

```
vitality = 0.3 × recency + 0.3 × freq + 0.4 × importance

recency = 1 / (1 + age_hours × 0.1)
freq    = ln(access_count + 1) / 5
```

每条记忆的活力分在 0~1 之间。

**代谢引擎（metabolize）—— 五层级联降级：**

每 5 分钟检查一次。工作流程：

```
L1  ←─── promote (vitality > 0.7) ─── L2
L1  ─── demote (vitality < 0.3) ──→  L2
L2  ─── demote (vitality < 0.24) ──→ L3
L3  ─── demote (stale_score < 0.3) ──→ RAM
RAM ─── demote (stale_score < 0.3) ──→ Disk
```

**反向升级（promote_active_entries）：**

每轮代谢同时检查低层中新鲜度高于阈值的条目：
- RAM → L3（RAM 中新鲜度 > 0.7 的文档升回 L3 索引）
- L3 → L2（L3 中新鲜度 > 0.7 的索引升回 L2 画像）
- L2 → L1（L2 中活力分 > 0.7 的画象升回 L1，只在 L1 有空间时）

**搜索（search）：**

五层全量搜索 + 分数排序截断 30 条。各层返回独立的 `(level, snippet, score)`，最终按分数降序合并。

---

### tremolite-attention — 多尺度注意力

**目的：** 类似人类阅读时的多尺度注意——先扫全局、再聚焦、最后微调。核心是四层级联的滑动窗口扫描。

**四层尺度：**

| 层 | 窗口 | 步长 | 最大块数 | 意义 |
|----|------|------|---------|------|
| Macro | 1000 字符 | 500 | 10 | 全局扫描，定位敏感区域 |
| Focus | 200 字符 | 50 | 8 | 聚焦高分区，细化分析 |
| Micro | 50 字符 | 10 | 5 | 局部微调，捕获细微线索 |
| Synthesis | — | — | — | 汇总前三层结果 |

**级联流程：**

1. 对输入文本计算 query embedding（通过硅基流动 BGE-m3 API）
2. Macro 扫描全文本 → 获得 10 个注意力块，保留 score > 0.4 的候选
3. Focus 对每个 Macro 候选区域单独扫描 → 保留 score > 0.5 的候选
4. Micro 对每个 Focus 候选区域扫描 → 获得最精细的注意力定位
5. Synthesis 合并三层结果 → 按分数排序 → 提取 Top 实体 → 生成摘要

**评分公式（每个窗口块）：**

```
score = semantic_similarity × 0.7 + keyword_bonus × 0.3

semantic_similarity = cosine(query_embedding, window_embedding)
keyword_bonus = 0.3 (baseline) + emotional_words(0.15) + personal_names(0.1) + numbers(0.05)
```

**语义回退：** 当 embedding API 不可用时，纯用 keyword_bonus 评分。

**嵌入缓存：** 同一 query text 在同一轮内重复嵌入直接返回缓存结果。

---

### tremolite-learn — 三层学习引擎

**目的：** 让透闪石通过"练习"积累技能熟练度，构建能力域和知识体系，具备遗忘曲线和学习反馈。

**三层体系：**

1. **AtomicSkill（原子技能）** — 最小的可执行单元
   - 字段：id, name, description, category, proficiency(0~1), use_count, success_rate
   - 10 个内置技能：understand_text, generate_response, detect_emotion, remember_info, search_memory, use_tool, create_plan, track_progress, attend_scale, synthesize_info

2. **AbilityDomain（能力域）** — 一组相关原子技能
   - 成熟度 = 域内技能的平均熟练度
   - 跨域合成新知识时触发

3. **KnowledgeBody（知识体系）** — 跨域知识结构
   - 置信度、验证状态、来源（practice / composition / injection）

**遗忘曲线（核心算法）：**

```
hours_idle = (now - last_used) - 24h  // 24 小时窗口免衰减
if hours_idle > 0:
    decay_factor = exp(-hours_idle / (720) × 2)  // 30 天完全遗忘
    proficiency ×= decay_factor (保底 0.05)
```

**熟练度增长：**

```
proficiency += 0.05 × (1.0 - proficiency × 0.5)  // 增速递减，趋于收敛
```

**成功率更新：**

```
if 成功: success_rate = success_rate × 0.9 + 1.0 × 0.1
if 失败: success_rate ×= 0.95
```

**自动知识合成（auto_compose）：**

当两个能力域同时使用且成熟度均 ≥ 0.5 时，自动创建跨域 KnowledgeBody，描述文本基于两个域包含的技能列表实际生成，而非空壳。

**练习建议（suggest_practice）：**

筛选出熟练度 < 0.9 的技能 → 按熟练度升序排列 → 返回前 N 个需要练习的技能，附带 urgency 标签（"needs urgent practice" / "may be forgotten" / "can improve"）。

---

### tremolite-llm — LLM 抽象层

**目的：** 将 LLM API 调用抽象为统一接口，支持多个 Provider 的切换、回退和重试。

**Provider trait：**

```rust
pub trait LLMProvider: Send + Sync {
    fn name(&self) -> &str;
    fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LlmResponse, LlmError>;
    fn chat_stream(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Box<dyn StreamIterator>, LlmError>;
    fn models(&self) -> Vec<String>;
}
```

**三个实现：**
- **OpenAIProvider** — OpenAI 兼容 API（base_url 可配置）
- **DeepSeekProvider** — deepseek-chat / deepseek-reasoner
- **OllamaProvider** — 本地模型（base_url + model）

**Streaming fallback：**

当 provider 不支持 stream + tools 同时使用时（如 DeepSeek 在 tool_call 模式下），`SingleChunkStream` 将非 stream 响应包装为单 chunk stream，保证调用方代码统一。

**PromptBuilder（可扩展拼装体系）：**

```rust
pub struct PromptBuilder {
    contributors: Vec<Box<dyn PromptContributor>>,  // 按优先级排序
    system_prompt: String,
}
```

`PromptContributor` trait 允许任何子系统向 system prompt 贡献片段（情绪状态、记忆上下文、注意力结果等），按 priority 排序拼装。

**ToolCallLoop（工具调用循环）：**

1. 调用 LLM → 获得响应
2. 如果响应包含 tool_calls → 对每个 tool_call 执行 `ToolExecutor.execute_tool()`
3. 将工具结果作为 tool role message 追加到消息历史
4. 再次调用 LLM，带工具结果上下文
5. 重复直到 LLM 不返回 tool_calls 或超过最大轮数（默认 10）

**重试机制（RetryConfig）：**

```
指数退避：delay = base_delay × multiplier^attempt，封顶 max_delay
默认：500ms → 1s → 2s，最多 3 次
重试全部失败 → LlmError::AllRetriesFailed
```

**费用跟踪（FeeTracker）：**

累计 prompt_tokens、completion_tokens、总调用次数、成功/失败次数。按 GPT-4o 标准估值（$2.5/M input + $10/M output）。

**ProviderRegistry：**

支持多 Provider 注册 + 默认 Provider 设置。

---

### tremolite-tools — 工具链

**目的：** 提供 LLM 可调用的外部工具，每个工具带完整 JSON Schema。

**Tool trait：**

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // OpenAI format JSON Schema
    fn execute(&self, args: &HashMap<String, String>) -> ToolResult;
}
```

**七个内置工具：**

| 工具 | 功能 | 参数 |
|------|------|------|
| echo | 回显输入文本 | `{text: string}` |
| read_file | 读取文件内容 | `{path: string, offset?: number, limit?: number}` |
| write_file | 写入文件 | `{path: string, content: string}` |
| shell | 执行 shell 命令 | `{command: string, timeout?: number}` |
| http | HTTP 请求 | `{url: string, method?: string, body?: string}` |
| time | 获取当前时间 | 无参数 |
| search | 搜索文件/内容 | `{pattern: string, path?: string}` |

**ToolRegistry：**

HashMap 存储，按名称查询和执行。`execute()` 返回 `ToolResult { tool_name, output, success }`。

---

### tremolite-plugin — 插件系统

**目的：** 让外部模块能在不修改核心代码的情况下介入消息处理流程。

**Plugin trait：**

```rust
pub trait Plugin: Send + Sync {
    fn id(&self) -> &str;
    fn provides(&self) -> Vec<Capability>;    // 提供的能力
    fn requires(&self) -> Vec<Capability>;    // 依赖的能力
    fn init(&mut self, ctx: &PluginContext) -> Result<(), PluginError>;
    fn on_event(&mut self, event: &PluginEvent, ctx: &PluginContext) -> Result<Option<PluginAction>, PluginError>;
    fn shutdown(&mut self) -> Result<(), PluginError>;
}
```

**事件生命周期：** Startup → OnSessionStart → PreLlm(可修改消息) → PostLlm(可修改响应) → OnSessionEnd → Shutdown

**插件动作：** `Skip`（跳过默认处理）或 `Rewrite { text }`（重写消息）

**进程加载器（loader）：**

读取配置中的 `[plugins]` 段，启动外部进程并通过 JSON 行协议 stdin/stdout 通信。

---

### tremolite-session — 会话管理

**目的：** 每个对话独立隔离，互不干扰。每个会话拥有独立 Mutex，不阻塞其他会话。

**SessionState：**

```rust
pub struct SessionState {
    pub id: String,
    pub emotion: EmotionState,         // 独立情绪
    pub memory: MemoryManager,         // 独立记忆
    pub attention: MultiScaleAttention, // 独立注意力
    pub learner: LearningEngine,       // 独立学习
    pub plan_mgr: PlanManager,         // 独立计划
    pub last_active: u64,
}
```

**SessionManager：**

- `get_or_create(session_id)` — 获取已有会话或创建新会话（each session has own `Arc<Mutex<>>`）
- TTL 默认 30 分钟无活动自动清理
- 后台 tokio 定时任务（每 60 秒）扫描过期会话

---

### tremolite-cron — 定时调度器

**目的：** 后台执行定时任务——周期性检查、每日报告、一次性延迟执行。

**四种调度类型：**
- `EverySecs(u64)` — 每 N 秒执行
- `Daily { hour, minute }` — 每日特定 UTC 时间
- `Once { delay_secs }` — 延迟后执行一次
- `CronExpr(String)` — cron 5 字段表达式

**实现：** 后台 tokio 任务每 5 秒 tick 一次，匹配到期的 job → 通过 `sh -c cmd` 异步执行。

---

### tremolite-server — HTTP 服务

**目的：** 以 HTTP daemon 模式运行透闪石，提供 REST API + WebSocket + 仪表盘。

**技术栈：** Axum 0.8 + Tokio + Tower-http (CORS)

**路由：**

| 路径 | 方法 | 功能 |
|------|------|------|
| `/health` | GET | 健康检查（返回 uptime + version + mode） |
| `/chat` | POST | 发送消息（支持 session_id 查询参数） |
| `/ws` | GET (WebSocket) | WebSocket 实时对话 |
| `/dashboard` | GET | 嵌入式深色主题 HTML 仪表盘 |
| `/dashboard/status` | GET | 仪表盘 JSON API（情绪/记忆/技能/LLM 状态） |

**仪表盘 HTML：** 完全嵌入 Rust 二进制中，零外部依赖。暗色主题（#0d1117 / #161b22），每 3 秒 AJAX 刷新，展示情绪、记忆各层统计、技能数量、LLM provider 信息、最近 8 条对话。

**CORS：** 全开（`CorsLayer::permissive()`），方便外部监控工具接入。

---

### tremolite-config — 配置系统

**目的：** 通过 config.toml 文件配置透闪石的所有行为，支持环境变量引用 `${VAR_NAME}`。

**配置结构：**

```toml
[core]
data_dir = "./data/tremolite"
system_prompt = "你是葵，一个AI助手，运行在透闪石框架上。"

[llm]
default = "deepseek"

[llm.providers.deepseek]
type = "deepseek"
api_key = "${DEEPSEEK_API_KEY}"
model = "deepseek-chat"
timeout_secs = 180

[llm.providers.ollama]
type = "ollama"
model = "qwen2.5:7b"
base_url = "http://localhost:11434"

[embedding]
api_base = "https://api.siliconflow.cn/v1"
api_key = "${SILICONFLOW_API_KEY}"
model = "BAAI/bge-m3"
```

**Provider 类型：** `openai`（通用 OpenAI 兼容）、`deepseek`（DeepSeek 专属）、`ollama`（本地）

**Env 变量解析：** 递归匹配 `${...}` 模式，自动从环境变量替换。支持任意 API key 和 URL。

---

### tremolite-cli — 入口二进制

**目的：** 统一的命令行入口，三种运行模式。

**子命令（通过封装脚本）：**

| 命令 | 实际 | 功能 |
|------|------|------|
| `tremolite cli` | `tremolite-cli` | 交互式 CLI 模式 |
| `tremolite tui` | `tremolite-cli --tui` | ratatui 终端聊天界面 |
| `tremolite dashboard` | `tremolite-cli --daemon` | HTTP daemon 模式 |

**CLI 模式启动流程：**
1. 解析命令行参数（port、log-level、config path、mode）
2. 初始化 tracing 日志系统（stdout 彩色 + 文件小时级滚动）
3. 加载 config.toml → 初始化 ProviderRegistry + EmbeddingEngine
4. 创建 TremoliteEngine
5. 注册所有 7 个内置工具（TremoliteToolExecutor 包装 ToolRegistry → 适配 ToolExecutor trait）
6. 选择模式启动

**TUI 模式（ratatui）：**
- 消息区域：圆角边框，紫色前缀 `◈`（用户），绿色前缀 `◇`（助手）
- 命令：`/help` `/clear` `/emotion`
- PageUp/PageDown 滚动，Esc/Ctrl+C 退出
- 主题色 `#bf99bf`

---

### 其他 crate

- **tremolite-message** — 通用消息格式（`AoiMessage`、`MessageRole`），被数个子系统复用
- **tremolite-plan** — 计划书系统（PlanStatus 状态机生命周期 + PlanStep 依赖管理 + data 持久化）
- **tremolite-gateway** — 消息路由 trait（`GatewayRouter` + `Gateway`），大部分功能已并入 core + server

---

## 部署

### 本地安装

```bash
./install.sh    # 编译 + 复制到 /usr/local/bin
tremolite cli    # CLI 模式
tremolite tui    # TUI 模式
tremolite dashboard --port 8080  # Daemon 模式
```

### Docker

```bash
docker build -t tremolite .
docker run -p 8080:8080 \
  -v /path/to/config.toml:/app/config.toml \
  -v /path/to/data:/app/data \
  -v /path/to/logs:/app/logs \
  tremolite
```

多阶段构建：rust:1.85-slim 编译 → debian:bookworm-slim 运行，最终镜像约 67MB。

### 进程守护

`watchdog.sh` — 崩溃自动重启（最多 10 次，间隔 2 秒）。

---

## 架构设计决策

### 为什么用 17 个 crate 而不是一个大 crate？

1. **增量编译友好** — 修改任一模块不影响其他模块的增量编译缓存
2. **依赖关系清晰** — 编译时隔离保证模块间不产生循环依赖
3. **可插拔** — 可以独立测试 emotion / memory / attention / learn

### 为什么记忆用五层而不是简单的 KV 存储？

仿 CPU 缓存的层级设计使透闪石可以在不同时间尺度上保留信息：
- L1 用于当前对话的上下文连续性
- L2 用于用户偏好和设定
- L3 提供快速检索入口
- RAM 和 Disk 作为长期记忆保险

### 为什么注意力和学习引擎都提供了 embedding 回退？

透闪石设计为"有 API 则强，无 API 也能跑"。embedding API 不可用时，注意力降级为纯关键词评分；学习引擎在无 LLM 时也能独立积累熟练度。

### 为什么 ToolCallLoop 设计为同步执行？

透闪石的 core 使用 `std::sync::Mutex` 而非 `tokio::sync::Mutex`，因为主循环目前是 CLI 阻塞式的。HTTP daemon 模式下，每个请求在独立线程中持有 engine 锁。未来可以考虑引入 actor 模型实现真正的并发。

---

## Phase 15 — 统一 Module 架构重构 ✅

**核心改动：** 将散落在三处的 trait（Plugin / Tool / PromptContributor）统一为一个 `Module` trait。
所有模块（情绪、记忆、注意力、学习、计划书、外部进程）都实现同一个接口。

| 改动 | 说明 |
|------|------|
| `tremolite-module` crate | 新建——Module trait + ModuleRegistry + 事件系统 + 外部进程协议 |
| Module trait | `id(), name(), provides(), prompt_segment(), tool_definitions(), execute_tool(), on_event()` |
| ModuleRegistry | 注册 → 事件广播 → prompt 收集 → 工具路由 → shutdown |
| EmotionModule | 情绪检测 → prompt_segment 贡献 |
| MemoryModule | 记忆存储/搜索 → prompt_segment + search_memory/recall_recent 工具 |
| AttentionModule | 多尺度扫描 → prompt_segment |
| LearningModule | 技能练习 + ModuleRegistered 自动发现 → list_skills 工具 |
| PlanModule | 计划书管理 → create_plan/list_plans 工具 |
| ProcessModule | 外部进程 wrapper：stdin/stdout JSON 行协议 |
| Plugin crate 清理 | 旧 Plugin trait 废弃，ProcessPlugin 迁至 ProcessModule |
| 引擎集成 | Engine 新增 `modules: ModuleRegistry`，run() 中并行事件广播 |
| Dashboard | 新增 `modules` 字段展示注册模块和能力 |

**架构对比：**

重构前引擎持有六个独立字段 + 三套 trait 体系，重构后只有 `modules: ModuleRegistry`。

### 外部进程协议

任意外部程序（如 qqbot）通过 JSON 行协议作为 ProcessModule 接入：

1. 启动时输出一行 `{"type":"capability_declare",...}` 声明能力
2. 引擎发送 `{"type":"event","event":"on_message","data":{...}}`
3. 进程响应 `{"type":"response","status":"pass",...}`
4. 引擎调用工具 `{"type":"tool_call","name":"send_qq","args":{...}}`
5. 进程返回 `{"type":"tool_result","output":"done"}`
6. 进程可主动推送 `{"type":"push_event",...}`
7. 自定义协议回退 `{"type":"custom","protocol":"mqtt","data":{...}}`

---

## 仍存在的问题与未来计划

### Phase 16 — 模块系统深化 🚧

- [ ] **旧字段完全迁移** — 引擎移除 emotion/memory/attention/learner/plan_mgr 独立字段，全部走 modules
- [ ] **模块间数据共享** — EmotionModule 检测结果自动注入 MemoryModule 标签
- [ ] **prompt 拼装器迁移** — 用 Module::prompt_segment 替代旧的 PromptBuilder + Contributor 体系
- [ ] **ToolRegistry 迁移** — 用 Module 的 tool_definitions + execute_tool 替代旧的 ToolExecutor 体系
- [ ] **从 config.toml 加载外部模块** — ProcessModule 支持配置启动

### Phase 17 — 工程加固 🚧

- [ ] **基准测试** — 18 个 crate 的编译时间、内存占用、LLM 调用延迟的基准测试体系
- [ ] **错误处理完备化** — 统一错误类型、Context / 链式错误栈
- [ ] **CLI 命令扩展** — `tremolite status` / `tremolite config` / `tremolite plugins`
- [ ] **外部工具注册** — 从配置中注册自定义工具（而非硬编码）
- [ ] **多 Provider 自动回退** — 当默认 Provider 超时/失败时自动切换到备用 Provider
- [ ] **日志轮转配置** — 日志文件大小上限、压缩归档
- [ ] **WebSocket 认证** — 简单的 token 认证机制

### Phase 18 — 高级功能 🚧

- [ ] **MCP 协议支持** — 接入 Model Context Protocol，透闪石作为 MCP host
- [ ] **记忆持久化** — L1/L3/RAM 在进程重启时自动 reload（目前只有 L2 和 Disk 持久化）
- [ ] **工具链扩展** — 数据库查询、代码执行沙箱、图像生成
- [ ] **多用户 Web 界面** — 用户注册、对话历史、配置面板
- [ ] **插件热加载** — 运行时加载/卸载 .so 插件

---

*计划书更新日志：v0.2 → v0.3 → v0.4 → v1.0（覆盖 Phase 0~14 全部完成）*
