# 透闪石 Tremolite —— 完整计划书 v0.3

> 一个真正可用的自主 AI agent 框架
> 献给神大人·琳玲 💞

---

## 已完成（5025 行 Rust）

### Phase 0 — 项目初始化 [✓]
### Phase 1 — 骨架搭建 [✓]
### Phase 2 — 情绪引擎 [✓]
### Phase 3 — 五层缓存记忆 [✓]
### Phase 4 — 多尺度注意力 [✓]
### Phase 5 — 计划书系统 [✓]
### Phase 6 — 学习引擎 [✓]
### Phase 7 — LLM 接入层 [✓]
### Phase 8 — 真正工具链 [✓]
### Phase 9 — 消息路由 [✓]
### Phase 10 — 真正的核心循环 [✓]

---

## Phase 11 — 编译与后端跑通 🚧

**目标：让透闪石能在神大人的电脑上真正跑起来**

### 11.1 修复编译错误 [在 NUC 完成 ✅ — Windows 待做 🚧]
- [x] NUC 上 rustup + gcc 安装
- [x] 修复 PluginEvent 缺少 Startup/Shutdown
- [x] 修复 mutable borrow 冲突（Rc<RefCell<>>）
- [x] 修复 WrapperExecutor 生命周期
- [x] 修复 tremolite-core/llm/tools 所有类型不匹配
- [x] 13 个 crate 全部 cargo build 通过，0 error
- [x] 二进制 tremolite-cli 输出成功（67MB）
- [ ] Windows 上交叉编译或通过 WinRM 直接编译

### 11.2 配置系统 [✅ 已完成]
- [x] 设计 config.toml 格式：provider 选择、API key、模型名、数据目录
- [x] 新建 `tremolite-config` crate
- [x] 支持三种 provider 类型：openai / deepseek / ollama
- [x] 支持 `${ENV_VAR}` 环境变量引用
- [x] Provider 从配置自动初始化并注入 ProviderRegistry
- [x] 无配置时降级运行（只用情绪回应）
- [x] config.example.toml 模板文件
- [x] 启动时加载配置并显示 provider 注册状态
- [x] 编译通过，启动验证成功

### 11.3 Provider 功能补全 [✅ 已完成]
- [x] 配置可自定义超时时间（OpenAI/DeepSeek/Ollama 各有默认值）
- [x] 三个 provider 全部添加 `with_timeout(secs)` builder 方法
- [x] config.toml 支持 timeout_secs 字段
- [x] RetryConfig 结构体 + 指数退避算法
- [x] FeeTracker 费用跟踪器（累计 token + 预估成本）
- [x] DeepSeek streaming 修复（工具调用时自动 fallback 到非 streaming）
- [x] Provider 切换验证：注册表切换 + 同一 prompt 不同 provider 输出一致性测试
- [x] 重试逻辑实际接入 ToolCallLoop（chat_with_retry + with_retry_config builder）

### 11.4 后端运行 [✅ 已完成]
- [x] 启动 CLI 测试完整流程：消息→情绪→记忆→LLM→工具→回复
- [x] 加入 `--daemon` 模式：后台运行，通过 HTTP API 交互
- [x] 健康检查端点 `GET /health`（返回 status/uptime/version）
- [x] POST /chat 端点（接收 {message: ...}，返回语言回复）
- [x] 日志系统（tracing + 文件输出 + `--log-level` 参数）
- [x] 进程守护：watchdog.sh（自动崩溃重启，最大次数限制）

---

## Phase 12 — 前端 Dashboard 🚧

**目标：让神大人能看到透闪石的心跳**

### 12.1 Dashboard 设计 [⏳ 待做]

### 12.2 后端 HTTP API [⏳ 待做]

### 12.3 前端实现 [⏳ 待做]

### 12.4 Dashboard 集成 [⏳ 待做]

---

## 当前状态总览（2026-06-06）

| Phase | 内容 | 状态 |
|-------|------|------|
| 0~10 | 骨架 + 模块全部 | ✅ 5025 行 |
| **11.1** | **修复编译** | **⬜→✅ 已完成（NUC）** |
| **11.2** | **配置系统** | **⬜→✅ 新完成** |
|| **11.3** | **Provider 补全** | **⬜→✅ 新完成** |
|| **11.4** | **后端运行** | **⬜→✅ 新完成** |
| 12 | 前端 Dashboard | ⬜ |

**下一步建议：**
1. 继续 11.3 Provider 功能补全（streaming / 重试 / 费用统计）
2. 做 Windows 编译环境（两个都做的那部分）
3. 或者直接进 Phase 12 前端 Dashboard

神大人选哪个喔💞
