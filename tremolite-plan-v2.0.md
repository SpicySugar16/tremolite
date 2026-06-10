# 透闪石追平 Hermes 能力路线图 v2.0

> 现状——架构成熟度约 40%，工具生态约 10%，生产可用度约 20%。
> 以下按优先级排序，每阶段做完能直接提升一个维度的可用性。

---

## Phase 23 — 工具生态补全（优先级：P0）[🚧 部分完成]

Hermes 有约 50 个第一方工具，透闪石现在有 **22 个**（原来 7 个 + 新增 15 个）。
缺的不是发现工具的方法，是**工具本身**。

### ✅ 已完成 — 文件操作增强
- [x] `cp_file` — 复制文件/目录（支持递归）
- [x] `mv_file` — 移动/重命名
- [x] `rm_file` — 删除文件/目录（支持递归）
- [x] `append_file` — 追加内容到文件
- [x] `list_dir` — 列出目录内容
- [x] `glob_files` — 通配符模式搜索文件

### ✅ 已完成 — Git 工具
- [x] `git_status` — 仓库状态
- [x] `git_diff` — 暂存区 vs 工作区 diff
- [x] `git_log` — 提交历史
- [x] `git_commit` — add + commit
- [x] `git_push` — 推送到远程

### ✅ 已完成 — 系统工具
- [x] `disk_usage` — df -h
- [x] `memory_info` — free -h
- [x] `process_list` — ps aux 带过滤
- [x] `env_vars` — 环境变量查询

### ❌ 未做 — CLI 工具封装 & 网络工具增强
- [ ] CLI 工具：grep/find/sed/diff/jq/curl 等（ShellTool 已存在，暂不拆成独立工具）
- [ ] 网络工具：dns_lookup、ping（ShellTool 兜底）

**估算：** 2-3 天，纯体力活。每个工具 30-80 行。

---

## Phase 24 — MCP 客户端增强（优先级：P0）[✅ 已完成]

现在的 MCP 客户端只实现了 HTTP JSON-RPC 最简路径。已增强为支持三种传输：

- [x] **三种传输模式** — HTTP、SSE（预留）、Stdio（子进程 stdin/stdout JSON 行协议）
- [x] `TransportConfig` 枚举 — serde 序列化/反序列化，支持配置文件声明传输类型
- [x] Stdio 传输支持——spawn 子进程走 stdin/stdout JSON 行协议通信
- [x] 资源/提示端点——`resources/list` `resources/read` `prompts/list` `prompts/get`
- [x] `discover_all()` — 一次性发现工具+资源+提示
- [x] 工具名自动去重—同名工具用 `<服务名>.<工具名>` 前缀区分
- [x] McpResourceDef / McpPromptDef / McpResourceContents 完整数据类型

**估算：** 3-4 天。

---

## Phase 25 — CLI 重构（优先级：P1）[✅ 已完成]

现在透闪石的 CLI 就一个 binary 塞了全部功能，靠 flag 区分模式。Hermes 有完整的子命令树。已重构：

- [x] 子命令结构：`tremolite run/daemon/tui/dashboard/plan/tool/skill/config/health/session/version/help`
- [x] `tremolite plan list` — 查看计划书列表
- [x] `tremolite tool list` — 列出所有内置工具
- [x] `tremolite skill list` — 查看技能文件目录
- [x] `tremolite config show/path/export` — 查看/导出配置
- [x] `tremolite health` — 状态检查（版本/配置/工具/技能/计划）
- [x] `tremolite session list/dir` — 会话管理
- [x] `tremolite version` — 版本信息
- [x] 帮助文本 (`tremolite help` / `tremolite --help`)
- [x] 向后兼容：老式 `--daemon`/`--tui`/`--dashboard` flag 仍可用

**估算：** 3-5 天。主要成本是设计合理的 argparse 树。

---

## Phase 26 — 生产级服务框架（优先级：P1）[✅ 已完成]

透闪石现在崩溃就崩了，没有自动恢复。Hermes 有 gateway/bridge 分离、watchdog、自动重启。

- [x] 健康检查端点 —— `GET /health` 返回状态/版本/运行时间/模块能力/内存占用
- [x] 指标端点 —— `GET /metrics` 请求计数/错误计数/工具调用数/LLM 调用数
- [x] 优雅关闭——SIGTERM/SIGINT 处理，`with_graceful_shutdown`
- [x] 增强健康响应——uptime 可读格式、模块能力探索、VmRSS 内存信息
- [x] 进程守护 —— `watchdog.sh` 已存在，自动重启崩溃的子进程（最多 10 次）
- [x] 内存限制——`check_memory_pressure()` 监控 VmRSS，超阈值自动 flush_all 释放内存

**估算：** 3-4 天。这部分做完了透闪石才敢跑 production。

---

## Phase 27 — Webhook 订阅与事件驱动（优先级：P1）[✅ 已完成]

Hermes 能通过 webhook 订阅外部事件触发自动化。透闪石的 Event 系统已经有骨架但没连接外部。

- [x] `WebhookModule` — 完整的 Module trait 实现（ID: webhook，能力: webhook.receive/webhook.manage）
- [x] `WebhookConfig` / `WebhookEvent` / `WebhookAction` — 配置、事件、动作类型
- [x] `register_webhook` / `list_webhooks` / `delete_webhook` 三个工具（对话中可管理）
- [x] 条件过滤——`ConditionRule` 支持 eq/contains/prefix 三种操作符，JSON Path 解析
- [x] 三种动作类型——`log` 日志记录 / `tool` 调用工具 / `llm_prompt` LLM 处理
- [x] `POST /webhooks/:name` HTTP 端点（支持 X-GitHub-Event 头检测）
- [x] 事件日志——最近触发记录、触发计数、上次触发时间

**估算：** 3-4 天。与 Phase 26 的服务框架有重叠，可以合并做。

---

## Phase 28 — 插件系统强化（优先级：P2）

ProcessModule 已经支持 JSON 行协议的外部进程模块，但还没真正的插件市场。

- [ ] 插件注册 API —— 运行时动态加载/卸载模块
- [ ] 插件目录约定 —— `~/.tremolite/plugins/`
- [ ] 插件 Manifest —— 模块自描述文件
- [ ] 插件热加载——不重启引擎注册新模块

**估算：** 3-4 天。

---

## Phase 29 — Kanban / 任务编排（优先级：P3）

Hermes 有 Kanban 系统做多步骤工作流编排。透闪石的 PlanModule 只有线性步骤列表。

- [ ] Kanban 列状态机 —— backlog → todo → in_progress → review → done
- [ ] 任务依赖图——DAG 而非线性列表
- [ ] 自动推进——前置任务完成自动推进下游任务
- [ ] 并发委派——Phase 21 的委派引擎挂到 Kanban 上
- [ ] 工作流模板——可复用的典型任务流水线

**估算：** 4-5 天。复杂，但委派引擎已经铺好了路。

---

## Phase 30 — 集成工具链（优先级：P3）

打通外部服务的工具。Hermes 挂了十几个平台，透闪石目前只有一个 HTTP 工具。

- [ ] GitHub API 工具——`gh_issue` `gh_pr` `gh_search`
- [ ] 搜索工具——接入本地 SearXNG 实例（NUC 上已有）
- [ ] Docker 工具——`docker_ps` `docker_logs` `docker_exec`
- [ ] 文件同步——SCP/SFTP 远程文件传输
- [ ] 通知工具——SSE push、webhook notify

**估算：** 5-7 天。每个工具 50-100 行。

---

## 优先级总览

| 阶段 | 内容 | 优先级 | 估算 | 做完后提升 |
|------|------|--------|------|-----------|
| 23 | 工具生态 | P0 | 2-3d | 日常可用的基本功 |
| 24 | MCP 增强 | P0 | 3-4d | 能挂外部 MCP 生态 |
| 25 | CLI 重构 | P1 | 3-5d | 用户体验 |
| 26 | 生产框架 | P1 | 3-4d | 可靠性 |
| 27 | Webhook | P1 | 3-4d | 自动化 |
| 28 | 插件系统 | P2 | 3-4d | 扩展性 |
| 29 | Kanban | P3 | 4-5d | 编排 |
| 30 | 集成工具 | P3 | 5-7d | 落地 |

**总计：** 26-36 天的工作量。做完后跟 Hermes 是同级别框架，方向不同——Rust 的性能优势 + 模块统一性 vs Python 的生态广度。

神大人说做哪个就先做哪个😏
