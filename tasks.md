# 透闪石 Tremolite — 开发路线图

## 已完成

### Phase 0~10：核心骨架 ✅
| # | 阶段 | 状态 |
|---|------|------|
| 1 | 项目初始化 — Rust workspace + crate 结构 | ✅ |
| 2 | 骨架搭建 — Plugin trait + 消息协议 + 核心循环 trait | ✅ |
| 3 | 情绪引擎 — 八维情绪向量 + 复合 + 风格映射 | ✅ |
| 4 | 五层缓存记忆(数据结构) — L1~L3 + RAM + Disk | ✅ |
| 5 | 多尺度注意力(管线) — Macro→Focus→Micro→Synthesis | ✅ |
| 6 | 计划书系统 — Plan/PlanStep + 生命周期 | ✅ |
| 7 | 学习引擎(三层结构) — AtomicSkill→AbilityDomain→KnowledgeBody | ✅ |
| 8 | LLM 接入层 — Provider 抽象 + Prompt + ToolCallLoop + Streaming | ✅ |
| 9 | 工具链 — 文件/Shell/HTTP/Time/Search | ✅ |
| 10 | 消息路由 — GatewayRouter + CliGateway | ✅ |
| 11 | 核心循环 — Perceive→Reason→Act→Express + PluginEvent | ✅ |

### Phase 11~12：运行与前端 ✅
| # | 阶段 | 状态 |
|---|------|------|
| 1 | 编译修复(NUC) — 13 crate 0 error | ✅ |
| 2 | 配置系统 — config.toml + 环境变量 + 三种 provider | ✅ |
| 3 | Provider 功能补全 — Timeout + Retry + FeeTracker + 降级 | ✅ |
| 4 | 后端运行 — CLI 全链路 + Daemon + Health/Chat API + Logging + Watchdog | ✅ |
| 5 | Dashboard — 嵌入式深色 HTML + /dashboard + JSON API | ✅ |
| 6 | TUI 模式 — ratatui 终端聊天 | ✅ |
| 7 | 系统安装 — install.sh + tremolite 统一入口脚本 | ✅ |

### Phase 13：算法补完 ✅
| # | 阶段 | 状态 |
|---|------|------|
| 1 | 记忆五层流转 — L1→L2→L3→RAM→Disk + promotion + 集成测试 | ✅ |
| 2 | 语义注意力 — 硅基 BGE-m3 embedding + 关键词降级 + prompt 注入 + 测试 | ✅ |
| 3 | 学习反馈环 — 遗忘曲线 + 引擎消费 + auto_compose + 记忆写入 | ✅ |

## 进行中

### Phase 14：生产化 (Production Hardening)
把 demo 变成能用的 agent。

**优先级分级：P0=不做等于白做，P1=从demo到工具，P2=锦上添花**

| # | 任务 | 优先级 | 状态 | 说明 |
|---|------|--------|------|------|
| 1 | 对话历史回填 | **P0** | ✅ | MemoryManager 记忆喂进 conversation_history，LLM 终于有上下文 |
| 2 | 崩溃恢复 | **P0** | ✅ | 每轮对话后 autosave(L2/学习/计划)，启动时自动 reload |
| 3 | 异步化 | **P1** | ✅ | axum/tokio 替代 tiny_http，支持 WebSocket |
| 4 | 工具 schema 自动生成 | **P1** | ✅ | 从 ToolDef 自动生成 JSON Schema 给 LLM，7个工具全覆盖 |
| 5 | Session 管理 | **P2** | ⬜ | 多用户/多会话隔离 |
| 6 | 插件系统 | **P2** | ⬜ | 热加载 + 注册表 + 生命周期管理 |
| 7 | Cron 实现 | **P2** | ✅ | 真正的 cron 调度器，支持 EverySecs/Daily/Once/CronExpr |
| 8 | Docker 化部署 | **P2** | ✅ | Dockerfile + compose + .dockerignore |

## 统计
- 代码量：~7158 行 Rust（27 个源文件）
- 工作区 crate：14 个
- 单元测试：~50 个（1 个预存失败：plan/test_search）
