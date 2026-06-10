# 透闪石 Tremolite —— 完整计划书 v0.4

> 一个真正可用的自主 AI agent 框架
> 献给神大人·琳玲 🌫️

---

## 诚实的状态总览

| Phase | 内容 | 状态 |
|-------|------|------|
| 0~1 | 项目初始化 + 骨架搭建 | ✅ |
| 2 | 情绪引擎 | ✅ |
| 3 | 五层缓存记忆 | ⚠️ 结构写全了，但代谢循环没接、RAM/Disk 只进不出 |
| 4 | 多尺度注意力 | ⚠️ 四层管线搭好了，核心算法是 16 个关键词的硬编码匹配 |
| 5 | 计划书系统 | ✅ |
| 6 | 学习引擎 | ⚠️ 三层技能体系结构完整，但 practice 只是+0.05 计数器，反馈环没接 |
| 7 | LLM 接入层 | ✅ Provider 抽象 + Prompt 拼装 + Streaming + ToolCallLoop |
| 8 | 工具链 | ✅ 文件/Shell/HTTP/Time/Search 五工具 |
| 9 | 消息路由 | ✅ Gateway 抽象 + CLI 通道 |
| 10 | 核心循环 | ✅ Perceive→Reason→Act→Express |
| 11 | 编译与后端跑通 | ✅ 编译 0 error + Daemon + Health API + Chat API + Logging + Watchdog |
| 12 | Dashboard | ✅ 嵌入式深色主题 HTML + /dashboard/status 3秒刷新 |
| — | TUI 模式 | ✅ ratatui 终端聊天 + 命令 + 滚动 |
| — | 系统安装 | ✅ install.sh + tremolite 统一入口脚本 |

---

## ✅ 已完成模块（5025+ 行 Rust）

### Phase 0 — 项目初始化
- Rust workspace + 14 crate 结构
- 基础依赖配置，Cargo.toml 依赖链
- README

### Phase 1 — 骨架搭建
- Plugin trait + PluginEvent + PluginError
- AoiMessage 消息协议
- 核心循环 trait
- Tool trait + ToolRegistry
- CLI 交互入口

### Phase 2 — 情绪引擎
- 八维情绪向量
- 关键词检测 + 时间衰减
- 复合情绪合成
- 风格映射（tone_map）
- 包装为原生插件

### Phase 3 — 五层缓存记忆
**数据结构和接口完整，但代谢引擎未接入主循环，降级链路只走了 L1→L2。**
- L1 LRU 工作记忆（VecDeque，50 条）
- L2 LFU 画像记忆（HashMap，200 条，JSON 持久化）
- L3 备忘索引（标签 + 时间戳搜索）
- RAM 朴素全文搜索（分词匹配 + 截断片段）
- Disk 冷归档（JSONL 文件，自动清理）
- **代谢引擎**：vitality_score 公式写好了（时效30% + 频率30% + 重要度40%），evaluate() 能给出升降级建议
- **未接入**：`core/lib.rs` 的 `run()` 主循环从未调用 `metabolize()`，RAM/Disk 在 `remember()` 中从未被写入

### Phase 4 — 多尺度注意力
**四层管线结构完整，但核心算法是指数级缩减的关键词匹配。**
- Macro→Focus→Micro→Synthesis 级联扫描
- 滑动窗口 + 截断
- `calc_attention_score`：16 个情感词 + 5 个人名 + 数字检测的硬编码匹配
- `extract_known_entities`：13 个硬编码字符串的 `text.contains()`
- 无 TF-IDF，无 embedding，无语义理解，无可学习权重

### Phase 5 — 计划书系统
- Plan + PlanStep 数据结构
- 生命周期状态机（Draft→InProgress→Completed→Cancelled）
- 步骤依赖管理
- checklist 手册生成

### Phase 6 — 学习引擎
**三层技能结构完整，但反馈环没接——数据跑完引擎不看。**
- AtomicSkill → AbilityDomain → KnowledgeBody 分层
- `practice()`：每次 +0.05 proficiency，无遗忘曲线
- `auto_compose()`：创建空壳 KnowledgeBody，description 留空
- `suggest_practice()`：筛选低熟练度技能
- **未接入**：没有任何地方消费 skill 数据来调整引擎行为

### Phase 7 — LLM 接入层
- `LLMProvider` trait：chat() + chat_stream()，OpenAI/DeepSeek/Ollama 实现
- `PromptBuilder` + `PromptContributor` + `PromptContext`
- `ToolCallLoop`：tool_call → 执行 → 结果回传 → 继续
- Streaming 支持（SingleChunkStream fallback）
- RetryConfig + 指数退避
- FeeTracker 费用跟踪

### Phase 8 — 工具链
- 文件工具（read_file / write_file / search_files）
- Shell 工具（执行命令）
- HTTP 工具（GET/POST 请求）
- 时间工具（日期、计时）
- 搜索工具
- 工具自动注册到 LLM ToolDefinition

### Phase 9 — 消息路由
- GatewayRouter + Gateway trait
- CliGateway 实现
- 统一入站 → 出站消息格式

### Phase 10 — 核心循环
- 1 Perceive：拼 prompt（系统指令 + 情绪状态 + 工具定义 + 记忆上下文 + 用户输入）
- 2 Reason：调 LLM → 推理 / tool_call
- 3 Act：ToolCallLoop 多轮调用
- 4 Express：情绪风格注入 + 存入记忆 + 输出
- PluginEvent::Startup/PreLlm/PostLlm/Shutdown 事件钩子

### Phase 11 — 编译与后端跑通
- 13 个 crate 全部 cargo build 通过，0 error
- 二进制 tremolite-cli 输出成功（~67MB）
- 配置系统：config.toml + 环境变量引用 + 三种 provider
- Provider 超时 / 重试 / 费用跟踪
- DeepSeek streaming 降级修复
- CLI 全链路测试通过
- `--daemon` 模式：HTTP 服务器
- `GET /health` 健康检查
- `POST /chat` 对话 API
- tracing 日志系统（stdout + 文件滚动输出）
- watchdog.sh 进程守护

### Phase 12 — Dashboard
- 嵌入式深色主题 HTML 仪表盘
- `GET /dashboard` 提供页面
- `GET /dashboard/status` 返回 JSON（情绪/记忆/技能/LLM 实时状态）
- 3 秒自动刷新
- 零外部依赖（HTML 字符串内嵌在 Rust 二进制中）

### TUI 模式
- ratatui 终端聊天界面
- 消息区域（圆角边框、紫色用户前缀 `◈`、绿色助手前缀 `◇`）
- `/help` `/clear` `/emotion` 命令
- PageUp/PageDown 滚动
- Esc/Ctrl+C 退出
- 主题色：`#bf99bf`

### 系统安装
- `install.sh`：复制二进制 + 配置文件 + 创建数据目录
- `tremolite` 统一入口脚本（cli / tui / dashboard 子命令）
- 安装到 `/usr/local/bin`

---

## Phase 13 — 算法补完 🚧

**目标：把透闪石从"漂亮的空壳"变成"真的能跑"的 agent。**

核心问题就一个——**数据结构搭好了，但数据不在里面流动**。记忆、注意力、学习三个模块各自有一套漂亮的结构，但彼此之间的数据通路是断的。

### 13.1 记忆五层流转

**现状：** `remember()` 只写 L1 + 创建空 L3 索引。RAM 和 Disk 从未被写入。代谢引擎从未被调用。
**目标：** 记忆从 L1 开始，随其活力分自然降级到 L2→L3→RAM→Disk；用户交互使条目从低层重新提升回来。

| 步骤 | 内容 | 估算 |
|------|------|------|
| 13.1.1 | `remember()` 也在 RAM 中建立索引（写入 RamFullTextSearch） | 1h |
| 13.1.2 | `metabolize()` 接入 `core/lib.rs` 主循环的 `run()` 中，每轮对话后调用 | 0.5h |
| 13.1.3 | 补全 L2→L3 降级通路：MetabolismEngine 评估后实际写入 L3 索引 | 1.5h |
| 13.1.4 | 补全 L3→RAM 降级通路：超过阈值的 L3 条目移入 RAM 全文搜索 | 1h |
| 13.1.5 | 补全 RAM→Disk 降级通路：冷数据归档到 JSONL 文件 | 1h |
| 13.1.6 | 实现反向 promotion：命中搜索结果的低层条目自动提升一级 | 1.5h |
| 13.1.7 | L3 索引摘要生成：接入 LLM 或 extractive 摘要，替代当前的空字符串 | 2h |
| 13.1.8 | 集成测试：写入 100 条记忆，验证降级/升级全部通路 | 1h |

### 13.2 真正多尺度注意力

**现状：** `calc_attention_score` 是 16 个关键词的硬编码匹配，`extract_known_entities` 是 13 个字符串的 contains。
**目标：** 注意力机制能基于语义相似度打分，能识别未预定义的重要实体。

| 步骤 | 内容 | 估算 |
|------|------|------|
| 13.2.1 | 引入 embedding 依赖：接入本地 embedding 模型（或复用 LLM 的 logprobs） | 2h |
| 13.2.2 | 语义注意力评分：替换 `calc_attention_score` 为 embedding 余弦相似度 + TF-IDF 混合，与用户输入/记忆上下文的语义相关性 | 3h |
| 13.2.3 | 动态实体提取：NNP 词性标注 + 共指消解，替代静态列表 | 2h |
| 13.2.4 | 注意力结果注入 LLM prompt：高注意力片段实际出现在系统提示词中 | 1h |
| 13.2.5 | 集成测试：已知重要文本片段被正确标记高分数 | 1h |

### 13.3 学习反馈环

**现状：** `practice()` 只是计数器，`auto_compose()` 造空壳，没有任何技能数据被引擎消费。
**目标：** 技能熟练度真正影响引擎行为——高熟练度的技能优先调用，低熟练度的技能选择性使用；遗忘曲线使长期不用的技能自然衰减。

| 步骤 | 内容 | 估算 |
|------|------|------|
| 13.3.1 | 遗忘曲线：`practice()` 新增基于最后使用时间的衰减因子，取代纯加法 | 1h |
| 13.3.2 | 引擎消费 skill 数据：`process_with_llm()` 根据工具相关技能的成功率调整工具选择策略 | 2h |
| 13.3.3 | `auto_compose()` 真正的知识合成：两个域交叉使用时，生成有实际内容的 KnowledgeBody | 2h |
| 13.3.4 | 学习数据写入记忆：`suggest_practice()` 结果存入记忆系统 | 0.5h |
| 13.3.5 | 集成测试：反复练习某技能后引擎行为可观测变化 | 1h |

---

## Phase 14 — 插件系统真正运行 🚧（待定）

**这一 Phase 是否开展取决于 Phase 13 完成后透闪石的实际表现。**
- Plugin trait 已被外部 crate 注册
- 热加载 .so 插件
- 实现 2~3 个真实插件（如网络搜索、定时提醒）
- Plugin 对 prompt 的实际贡献能力

---

## 代码统计

```
Phase 0~6     3529 行 Rust  (数据结构层)
Phase 7~12   ~1500 行 Rust  (LLM + 工具 + 路由 + 运行 + Dashboard + TUI)
----------------------------------
总计         ~5025+ 行 Rust

Phase 13 预计      ~800~1200 行 Rust（算法补完）
```

---

## 开发规范

- 每个新 Phase 先在计划书中拆步骤，再动手
- 新加/修改功能时必须同步更新本计划书
- 写完算法必须写集成测试（不是单元测试，是真正跑通流程的测试）
- 不标 ✅ 直到集成测试通过
