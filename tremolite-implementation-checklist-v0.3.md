# 透闪石 Tremolite —— 执行清单 v0.3

> 可追踪的开发清单，每步带 [ ] / [~] / [✓] 标记
> 献给神大人·琳玲 💞

---

## Phase 0 — 项目初始化 [✓]

- [✓] 创建 Rust workspace
- [✓] 创建 12 个 crate 骨架
- [✓] 配置 workspace 依赖
- [✓] 编写 README

## Phase 1 — 骨架搭建 [✓]

- [✓] Plugin trait + PluginEvent + PluginError
- [✓] AoiMessage 消息协议
- [✓] CoreLoop trait（perceive / reason / act / express）
- [✓] Tool trait + ToolRegistry + EchoTool
- [✓] 交互式 CLI

## Phase 2 — 情绪引擎 [✓]（229行）

- [✓] 八维情绪向量
- [✓] detect_from_text 关键词检测
- [✓] decay 时间衰减
- [✓] dominant_emotion + composite_emotion
- [✓] style_from_emotion 风格映射
- [✓] 包装为插件
- [✓] CLI 集成
- [✓] 6 个测试

## Phase 3 — 五层缓存记忆 [✓]（958行）

- [✓] MemoryEntry + MemoryLevel + vitality_score
- [✓] L1 工作记忆 LRU
- [✓] L2 画像记忆 LFU + JSON
- [✓] L3 备忘索引
- [✓] RAM 全文搜索
- [✓] Disk 冷归档
- [✓] 代谢引擎
- [✓] MemoryManager 统一接口
- [✓] CLI 集成（/memory /search）
- [✓] 7 个测试

## Phase 4 — 多尺度注意力 [✓]（358行）

- [✓] 四层 AttentionScale
- [✓] 级联扫描
- [✓] calc_attention_score
- [✓] 实体提取
- [✓] 综合合成
- [✓] 4 个测试

## Phase 5 — 计划书系统 [✓]（491行）

- [✓] Plan + PlanStep + PlanStatus
- [✓] 状态机校验
- [✓] 步骤依赖管理
- [✓] generate_handbook
- [✓] CLI 集成（/plans /plan /handbook）
- [✓] 5 个测试

## Phase 6 — 学习引擎 [✓]（528行）

- [✓] AtomicSkill → AbilityDomain → KnowledgeBody
- [✓] 10 个内置技能
- [✓] practice 练习机制
- [✓] create_domain / create_knowledge / auto_compose
- [✓] suggest_practice
- [✓] CLI 集成
- [✓] 6 个测试

## Phase 7 — LLM 接入层 [✓]（771行）

- [✓] LLMProvider trait
- [✓] OpenAI provider
- [✓] DeepSeek provider
- [✓] Ollama provider
- [✓] ProviderRegistry
- [✓] PromptContributor trait（统一注入协议）
- [✓] PromptBuilder（按优先级合并）
- [✓] Streaming（OpenAI + DeepSeek SSE）
- [✓] ToolCallLoop（工具调用循环）
- [✓] ToolExecutor trait
- [✓] 5 个测试

## Phase 8 — 真正工具链 [✓]（418行）

- [✓] EchoTool
- [✓] ReadFileTool
- [✓] WriteFileTool
- [✓] ShellTool
- [✓] HttpTool（ureq）
- [✓] TimeTool
- [✓] SearchTool
- [✓] register_all 一键注册

## Phase 9 — 消息路由 [✓]（274行）

- [✓] InboundMessage / OutboundMessage
- [✓] Gateway trait
- [✓] CliGateway
- [✓] NullGateway
- [✓] GatewayRouter（注册 + 路由 + 广播）
- [✓] 5 个测试

## Phase 10 — 真正的核心循环 [✓]（321行）

- [✓] TremoliteEngine（全模块集成）
- [✓] 主循环：收消息→情绪→注意→记忆→学习→LLM→回复
- [✓] 有 provider 走 LLM，无 provider 走情绪降级
- [✓] ToolCallLoop 串联
- [✓] Plugin 事件通知
- [✓] 优雅关闭（flush 所有持久化）
- [✓] EmotionContributor（prompt 情绪注入）

---

## Phase 11 — 编译与后端跑通 [ ]

**11.1 修复编译错误**
- [ ] 在 Windows 上 `cargo build` 确认当前编译状态
- [ ] 修复 ToolCallLoop 中 WrapperExecutor 生命周期问题
- [ ] 修复 tremolite-tools ureq blocking 依赖
- [ ] 修复所有 crate 间类型兼容问题
- [ ] 修复所有 #[cfg(test)] 编译警告
- [ ] `cargo test --workspace` 全部通过

**11.2 配置系统**
- [ ] 设计 config.toml 格式
- [ ] config 结构体定义（Config, LlmConfig, ProviderConfig 等）
- [ ] ConfigLoader：搜索路径（~/.tremolite/config.toml → ./tremolite.toml）
- [ ] Provider 自动初始化（从配置读取 API key 和模型）
- [ ] 无配置降级运行
- [ ] 环境变量覆盖（TREMOLITE_API_KEY 等）

**11.3 Provider 补全**
- [ ] DeepSeek streaming 修复
- [ ] 超时重试
- [ ] Token 计数和成本估算

**11.4 后端运行**
- [ ] CLI 启动后完整流程测试
- [ ] `--daemon` 后台模式
- [ ] 健康检查 `GET /health`
- [ ] 日志输出（文件 + 终端）
- [ ] 进程守护

---

## Phase 12 — 前端 Dashboard [ ]

**12.1 设计**
- [ ] 技术栈确定：axum + 纯前端
- [ ] 页面布局设计
- [ ] 情绪雷达图设计
- [ ] 配色方案（#bf99bf 紫色/粉色）

**12.2 HTTP API**
- [ ] 在 tremolite-gateway 增加 HttpGateway（axum server）
- [ ] POST /api/chat — 对话接口
- [ ] GET /api/status — 全状态
- [ ] GET /api/emotion — 情绪八维
- [ ] GET /api/memory — 记忆统计
- [ ] GET /api/attention — 注意力结果
- [ ] GET /api/learn — 学习统计
- [ ] GET /api/plans — 计划书列表
- [ ] GET /api/logs — 调用日志
- [ ] GET /api/stream — SSE 流式推送

**12.3 前端**
- [ ] 单页 HTML（嵌入二进制或文件加载）
- [ ] Canvas 情绪雷达图
- [ ] 对话气泡界面
- [ ] 状态卡片 + 进度条
- [ ] 注意力可视化
- [ ] 深色主题 + 透闪石配色
- [ ] 纯原生，无外部依赖

**12.4 集成**
- [ ] CLI 启动时可选启动 HTTP
- [ ] 终端显示 URL
- [ ] 优雅关闭

---

## 统计

| Phase | 状态 | 行数 | 说明 |
|-------|------|------|------|
| 0 | ✅ | 项目初始化 |
| 1 | ✅ | 骨架搭建 |
| 2 | ✅ | 229 | 情绪引擎 |
| 3 | ✅ | 958 | 五层记忆 |
| 4 | ✅ | 358 | 注意力 |
| 5 | ✅ | 491 | 计划书 |
| 6 | ✅ | 528 | 学习引擎 |
| 7 | ✅ | 771 | LLM 接入层 |
| 8 | ✅ | 418 | 工具链 |
| 9 | ✅ | 274 | 消息路由 |
| 10 | ✅ | 321 | 核心循环 |
| 11 | 🚧 | — | 编译+配置+后端 |
| 12 | 🚧 | — | 前端 Dashboard |

总计：完成 11 个 Phase（含代码），剩余 2 个 Phase
Rust 代码：5025 行
