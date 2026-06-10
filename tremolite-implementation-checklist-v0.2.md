# 透闪石 Tremolite —— 执行清单 v0.2

> 可追踪的开发清单，每步带 [ ] / [~] / [✓] 标记
> 献给神大人·琳玲 💞

---

## Phase 0 — 项目初始化 [✓]

- [✓] 创建 Rust workspace
- [✓] 创建 12 个 crate 骨架
- [✓] 配置 workspace 依赖（serde, tokio, tracing 等）
- [✓] 编写 README

## Phase 1 — 骨架搭建 [✓]

- [✓] 实现 `aoi-plugin`：Plugin trait + PluginEvent + PluginError
- [✓] 实现 `aoi-message`：AoiMessage 数据结构
- [✓] 实现 `aoi-core`：CoreLoop trait（perceive / reason / act / express）
- [✓] 实现 `aoi-tools`：Tool trait + ToolRegistry + EchoTool
- [✓] 实现 `aoi-cli`：交互式命令行（/run /status /tools /help /exit）
- [✓] 测试：core_loop_default, core_loop_plugin_event

## Phase 2 — 情绪引擎 [✓]

- [✓] 八维情绪向量（joy, sadness, anger, fear, surprise, disgust, anticipation, trust）
- [✓] detect_from_text 关键词检测
- [✓] decay 时间衰减
- [✓] dominant_emotion + composite_emotion 复合合成
- [✓] style_from_emotion 风格映射
- [✓] 包装为插件（emotion/plugin.rs）
- [✓] CLI 集成（/emotion 显示心情）
- [✓] 测试（6 个：情绪检测×2、衰减、主导、复合、风格映射）

## Phase 3 — 五层缓存记忆 [✓]

- [✓] 核心数据类型（MemoryEntry, MemoryLevel, vitality_score）
- [✓] L1 工作记忆（LRU VecDeque, 容量50, 自动淘汰）
- [✓] L2 画像记忆（LFU + JSON 文件持久化, flush_on_drop）
- [✓] L3 备忘索引（标签+时间戳+摘要, 不存原文）
- [✓] RAM 全文搜索（朴素 FTS, 关键词匹配+时效加权）
- [✓] Disk 冷归档（JSONL, 上限50自动清理）
- [✓] 代谢引擎（活力分=30%时效+30%频率+40%重要度, 自动升降级）
- [✓] MemoryManager 统一接口（remember, search, metabolize, stats）
- [✓] CLI 集成（/memory 看层级统计, /search 全层级搜索）
- [✓] 测试（7 个：L1 LRU淘汰、L2持久化、L3索引、RAM FTS、Disk归档、活力分、代谢评估、管理器集成）

## Phase 4 — 多尺度注意力 [✓]

- [✓] 四层 AttentionScale（Macro/Focus/Micro/Synthesis）
- [✓] 级联扫描（宏观定位→聚焦放大→微观精炼）
- [✓] calc_attention_score（情感/人称/数字/标点密度加权）
- [✓] extract_known_entities 实体提取
- [✓] 综合合成（实体排名 + top regions + 压缩比）
- [✓] 测试（4 个：尺度窗口、分数计算、实体提取、全链路扫描）

## Phase 5 — 计划书系统 [✓]

- [✓] Plan + PlanStep + PlanStatus 全生命周期
- [✓] 状态机（can_transition_to 校验）
- [✓] 步骤依赖管理 + 可执行判断
- [✓] generate_handbook checklist 格式手册生成
- [✓] search / stats / flush
- [✓] CLI 集成（/plans /plan /handbook）
- [✓] 测试（5 个：生命周期、步骤、手册生成、搜索、进度计算）

## Phase 6 — 学习引擎 [✓]

- [✓] 三层体系：AtomicSkill → AbilityDomain → KnowledgeBody
- [✓] 10 个内置技能（understand_text, detect_emotion 等）
- [✓] practice 练习机制（熟练度增长、成功率更新）
- [✓] create_domain / create_knowledge / auto_compose
- [✓] suggest_practice 练习推荐
- [✓] search / stats / flush
- [✓] CLI 集成（/learn /practice /suggest）
- [✓] 测试（6 个：练习、掌握度、能力域、知识体、推荐、自动合成、搜索）

## Phase 7 — LLM 接入层 [ ]

**7.1 Provider 抽象**
- [ ] 定义 LLMProvider trait（chat, chat_stream, models）
- [ ] 实现 OpenAI provider（REST API, messages API, tool_calls）
- [ ] 实现 DeepSeek provider（兼容 OpenAI 格式）
- [ ] 实现 Ollama provider（本地模型）
- [ ] Provider 注册表（按名称选择）
- [ ] 配置文件（config.toml / config.json）

**7.2 统一 Prompt 拼装协议**
- [ ] 定义 PromptContributor trait（fn contribute(&self, ctx: &Context) -> Option<String>）
- [ ] Plugin trait 扩展：支持注册 PromptContributor
- [ ] PromptBuilder：按优先级合并所有贡献
- [ ] 拼装顺序：系统指令 → 记忆上下文 → 情绪注入 → 注意力高亮 → 计划书进度 → 工具定义 → 对话历史 → 用户输入
- [ ] 每个核心插件实现自己的贡献器：EmotionContributor, MemoryContributor, AttentionContributor, PlanContributor, LearnContributor

**7.3 Streaming**
- [ ] chat_stream 接口
- [ ] CLI streaming 显示
- [ ] 中断/停止机制

**7.4 工具调用循环**
- [ ] LLM 返回 tool_calls 解析
- [ ] 工具执行器（调用 ToolRegistry）
- [ ] 结果回传 LLM
- [ ] 多轮工具调用（上限控制）
- [ ] 超时和错误处理

## Phase 8 — 真正工具链 [ ]

- [ ] 文件工具（read_file, write_file, search_files）
- [ ] Shell 工具（执行命令, 超时控制）
- [ ] HTTP 工具（GET/POST, 超时, 错误处理）
- [ ] 时间工具（now, timer, datetime）
- [ ] 搜索工具（本地搜索, 联网搜索）
- [ ] 工具自动注册机制（init 时从配置加载）
- [ ] 工具调用日志 + 调试输出

## Phase 9 — 消息路由 [ ]

- [ ] Gateway trait 抽象（send, on_message, connect, disconnect）
- [ ] CLI 通道完善
- [ ] 入站消息标准化 → 核心循环
- [ ] 出站消息路由（按配置分发到指定通道）
- [ ] 通道注册表

## Phase 10 — 真正的核心循环 [ ]

- [ ] 改造 Perceive 层：prompt 拼装（系统+插件+记忆+工具+用户）
- [ ] 改造 Reason 层：调 LLM，解析响应
- [ ] 改造 Act 层：工具调用 + 结果回传 + 循环
- [ ] 改造 Express 层：情绪风格注入 + 记忆存储
- [ ] 完整端到端流程测试
- [ ] 错误处理 + 降级

---

## 统计

| 阶段 | 状态 | 说明 |
|------|------|------|
| Phase 0 | ✅ | 项目初始化 |
| Phase 1 | ✅ | 骨架搭建 |
| Phase 2 | ✅ | 情绪引擎 |
| Phase 3 | ✅ | 五层记忆 |
| Phase 4 | ✅ | 注意力 |
| Phase 5 | ✅ | 计划书 |
| Phase 6 | ✅ | 学习引擎 |
| Phase 7 | 🚧 | LLM 接入层 |
| Phase 8 | ⬜ | 真正工具链 |
| Phase 9 | ⬜ | 消息路由 |
| Phase 10 | ⬜ | 真正核心循环 |

总计：已完成 7 个 Phase，剩余 4 个 Phase
Rust 代码：3529 行
