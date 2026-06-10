# 透闪石 Tremolite —— 缺失能力补全计划 v1.2

> 接 v1.1，不重复已有内容，只写还空着的

---

## 差距总览

跟 Hermes 比，透闪石缺七个能力。按依赖优先级排列：

| # | 能力 | 依赖 | 说明 |
|---|------|------|------|
| 1 | 会话系统 | 无 | 最基础的——透闪石现在跑了就忘 |
| 2 | 消息通道 | 会话系统 | Hermes 接了 qqbot/NapCat/webhook/Telegram |
| 3 | 定时任务 | 无 | cron 调度器 crate 已存在但没接入主循环 |
| 4 | 反思系统 | 会话 + 记忆 | ChromaDB RAG + 个体画像 + 定期反思 |
| 5 | 技能系统 | 无 | skill_view/list/manage |
| 6 | 任务委派 | 消息通道 | spawn 子 agent |
| 7 | MCP 客户端 | 工具链 | 挂外部 MCP 服务当工具 |

---

## Phase 16 — 会话系统

**目的：** 透闪石现在每个请求从头跑，不知道"上一句说了什么"。会话系统让对话有记忆、能恢复。

**`tremolite-session` crate（已有骨架，需要补）：**

- [x] `SessionManager` — HashMap `<session_id, SessionState>`，每个会话独立 Mutex
- [x] `SessionState` — 持有独立 emotion + memory + attention 实例（当前透闪石引擎只持有一份全局的）
- [x] TTL 清理 — 30 分钟无活动自动回收，后台 tokio 任务每 60 秒扫描
- [x] `process_with_session()` — 引擎的 `run()` / `process_with_llm()` 改为按 session_id 隔离
- [x] CLI 模式会话名 — `--session <id>` 参数
- [ ] HTTP API `session_id` 查询参数 — `/chat?session_id=xxx`（未实施）

**注意：** 当前 `TremoliteEngine` 的 `modules: ModuleRegistry` 是全局的，每个 Module 内部状态也是全局的。要做到会话隔离，要么给每个会话 clone 一套 ModuleRegistry，要么 Module 内部实现 session 感知。**推荐后者**——Module 的 `on_event()` 拿 `EventContext`，里面放 `session_id`，模块自行按 session_id 区分状态。

**估算：** 3-5 天

---

## Phase 17 — 消息通道

**目的：** 透闪石现在只有 CLI 和 HTTP API。连 QQ 群聊都发不出去，更别说 Telegram 了。

**消息通道抽象：**

- [x] `Channel trait` — `send(chat_id, message) -> Result` / `recv() -> InboundMessage` / `name() -> &str`
- [x] `ChannelRegistry` — 类似 ProviderRegistry，注册后引擎自动消费
- [x] HTTP 回调通道 — 接收 webhook POST，透传消息给引擎处理
- [x] **QQ Bot** — 走 QQ 开放平台 API（Hermes 用的 qqbot adapter 可以参考但不直接搬，透闪石是 Rust）
- [x] **NapCat** — WebSocket 监听 NapCat 事件，走 Hermes 现有的 NapCat 桥接思路
- [x] Telegram — 可选，Bot API 调用

**架构：** `Channel` 在后台 tokio 任务中独立运行，收到消息后通过 `mpsc` 发送给引擎主循环。引擎处理后通过 `Channel.send()` 回复。

**估算：** QQ Bot 4-5 天，NapCat 3-4 天，Telegram 2 天

---

## Phase 18 — 定时任务

**目的：** 透闪石自己会醒来干活，不用等人叫。

**`tremolite-cron` crate（已有骨架，已接入）：**

- [x] 从 `config.toml` 加载 cron job 定义
- [x] 启动时 spawn 后台 tokio 任务
- [x] Job types：EverySecs / Daily / Once / CronExpr
- [x] Job action：通过引擎 `process_with_llm` 执行（给一个虚拟 session）
- [x] CLI 命令：`tremolite cron list` / `tremolite cron run <id>`

**依赖：** 会话系统（Phase 16）——因为 cron job 执行时需要一个隔离的会话上下文。

**估算：** 2-3 天

---

## Phase 19 — 反思系统

**目的：** 透闪石需要定期回头看自己记住的东西有没有问题，做抽象和去毒。

**这不是 Hermes 的 memory-rag 插件直接搬。Rust 生态没有 ChromaDB 原生绑定，需要设计透闪石自己的方案：**

- [x] **`tremolite-reflection` crate** — 反思引擎
- [x] 定期反思触发 — 每 N 条对话 / 定时触发
- [x] 摘要生成 — 调用 LLM 压缩对话片段为持久记忆
- [x] 重要性评分 — 关键词匹配 + LLM 评估
- [x] 个体画像 — 从记忆摘要中提取用户偏好/习惯的抽象描述
- [x] 向量检索 — 用本地的 `tremolite-attention` 的 embedding API（已有 BGE 接入）做语义检索，存最简单的 JSON 索引，不上 ChromaDB

**去毒清洗 — 归入 MemoryModule（反思只负责触发）：**
- [x] MemoryModule 新增 `fn decontaminate()` 方法
- [x] 内部操作：虚构检测（关键词扫描）、噪音删除（命令痕迹/短条目）、去重合并（按类型保留最新）
- [x] ReflectionModule 每 N 条对话触发 `broadcast(Event::Decontaminate)` → MemoryModule 响应执行
- [x] 反思负责触发和结果记录，去毒本身是 MemoryModule 的能力

**存储策略（按层级选择）：**
- 短期会话记忆（L1~L3级）：文件索引 + JSON，不向量化——量小、存取快
- 长期记忆检索（跨会话）：需要向量索引。用本地 embedding API（已有 BGE 接入）+ 轻量向量存储（JSON 索引 + cosine 扫描，或嵌入 `tinyvector`/`lancedb` 等嵌入式向量库，不上 ChromaDB 那样的独立服务）
- 反思画像：向量检索最合适（自然语言描述），与长期记忆共享同一向量索引
- 原则：哪层需要向量哪层上，不做全量向量化也不一刀切不用

**估算：** 5-7 天

---

## Phase 20 — 技能系统（SkillModule）

**目的：** 将原有的 LearningModule 并入技能系统。技能文件的加载、熟练度追踪、工具选择策略、遗忘曲线全在 SkillModule 里完成，学习引擎是技能系统的内部机制，不是外壳。

**合并方式：** 现有 `modules/learning.rs` 改名 `modules/skill.rs`，`LearningModule` 改名 `SkillModule`，保留原有学习引擎的全部能力（AtomicSkill、遗忘曲线、ability domain、auto_compose），加上技能加载和注入。

- [x] 技能文件格式 — Markdown + YAML frontmatter，存 `~/.tremolite/skills/`
- [x] 技能加载 — `SkillModule.inject_skill(path)` 读取文件，自动创建/更新 AtomicSkill 记录
- [x] 运行时注入 — `BuildPrompt` 事件时根据上下文选择技能注入 system prompt
- [x] `list_skills` / `view_skill` 工具 — 让 LLM 自己能查技能
- [x] 熟练度联动 — LLM 调用某技能时，`on_event(OnToolCall)` 已自动记录 practice，无需额外逻辑
- [x] 熟练度 < 0.3 → skill prompt 注入时附带「此技能尚未掌握，谨慎使用」
- [x] 熟练度 > 0.7 → 该工具在 `available_tools` 过滤中提升排序优先级
- [x] 遗忘曲线 — 沿用现有 LearningEngine 的 `exp(-hours_idle/720) × 2` 衰减

**估算：** 3-4 天

---

## Phase 21 — 任务委派

**目的：** 透闪石能 spawn 子实例并行处理任务。

- [x] 子进程 spawn — 启动独立的 `tremolite` 进程（或线程），传入 session_id
- [x] 隔离上下文 — 子进程有独立的内存空间，通过 JSON 行协议 stdin/stdout 通信
- [x] `delegate_task` 工具 — LLM 能调用的委派工具
- [x] 结果收集 — 主进程等待子进程完成，取回结果摘要

**依赖：** 消息通道（Phase 17）——因为子进程需要能回复。

**估算：** 4-5 天

---

## Phase 22 — MCP 客户端

**目的：** 挂外部 MCP（Model Context Protocol）服务当工具用。

- [x] MCP 协议实现 — 标准 HTTP/SSE 传输
- [x] `mcp_client` crate — 工具发现 + 调用
- [x] 配置加载 — 从 `config.toml` 读取 MCP server 列表
- [x] MCP 工具注册 — 动态注册到 engine 的工具列表

**学习引擎联动：** MCP 模块注册工具到 engine 时，LearningModule 的 `on_event(ModuleRegistered)` 已自动为每个工具创建 `tool.<name>` 技能记录，无需额外改动。
- ⚠️ 注意：MCP 工具失败率高时会因 `success_rate >= 0.3` 过滤被筛掉 → LLM 不再调用。如需保留关键工具（如文件读写），需在 LearningModule 中加白名单机制。

**估算：** 3-4 天
