# Tremolite

> A modular AI agent framework built on Rust, powered by [Artic Protocol](https://github.com/SpicySugar16/artic-protocol)

> [!WARNING]
> **Under Development** — This is an early-stage research project. A ready-to-download release with pre-built binaries will be available in a future version. For now, please build from source.
>
> **开发中** — 这是一个早期研究项目。后续版本将提供可直接下载的发行版。目前请从源码构建。

**English** · <a href="#chinese-version">中文</a>

![Tremolite Crate Architecture](./docs/assets/crate-architecture-en.svg?t=1)

## What is Tremolite?

Tremolite is a **modular AI agent framework** written in Rust. It follows the [Artic Protocol](https://github.com/SpicySugar16/artic-protocol) — a standard communication protocol for agent modules — enabling hot-swappable, language-agnostic module collaboration.

Think of it as a **reference implementation** of Artic Protocol: the engine discovers modules by their declared capabilities, routes requests by service name, and orchestrates tool execution, memory, emotion, planning, and LLM calls through a clean message-flow pipeline.

![Module Message Flow](./docs/assets/module-flow-en.svg?t=1)

## Architecture

Tremolite's workspace is organized into **5 layers**:

| Layer | Purpose | Crates |
|-------|---------|--------|
| **External Interface** | Entry points, protocol adaptation, channels | `tremolite-cli`, `tremolite-server`, `tremolite-dashboard`, `tremolite-plugin`, `tremolite-message`, `tremolite-channels` |
| **Cognition & Intelligence** | LLM integration, emotional state, memory, attention, learning | `tremolite-llm`, `tremolite-emotion`, `tremolite-attention`, `tremolite-learn`, `tremolite-reflection`, `tremolite-distiller`, `tremolite-self-learner`, `tremolite-compress` |
| **Core Engine** | Phase 10 main loop, session scheduler, module lifecycle | `tremolite-core` |
| **Planning & Tools** | Planning, tool execution, delegation, MCP, cron | `tremolite-plan`, `tremolite-tools`, `tremolite-delegation`, `tremolite-mcp`, `tremolite-cron` |
| **Infrastructure** | Config, persistent memory, session management | `tremolite-config`, `tremolite-memory`, `tremolite-session` |

## Quick Start

```bash
# Build all crates
cargo build

# Run tests
cargo test

# Start the CLI
cargo run -p tremolite-cli

# Start the HTTP server
cargo run -p tremolite-server
```

## Crate Overview

| Crate | Description |
|-------|-------------|
| `tremolite-core` | Core engine — Phase 10 main loop, session scheduler, module registry |
| `tremolite-plugin` | Plugin trait + module interface per Artic Protocol |
| `tremolite-message` | Message protocol — envelopes, routing, serialization |
| `tremolite-channels` | Multi-channel gateway — QQ Bot, NapCat, HTTP, WebSocket |
| `tremolite-session` | Session lifecycle management |
| `tremolite-config` | Configuration parsing and validation |
| `tremolite-cli` | CLI entry point — interactive shell |
| `tremolite-server` | HTTP/WebSocket server (Axum) |
| `tremolite-dashboard` | Admin web dashboard |
| `tremolite-llm` | LLM provider abstraction (OpenAI, DeepSeek, Ollama) |
| `tremolite-emotion` | Emotional state engine |
| `tremolite-memory` | 5-layer cached memory (RAG) |
| `tremolite-attention` | Multi-scale attention mechanism |
| `tremolite-learn` | Online learning engine |
| `tremolite-reflection` | Self-reflection and meta-cognition |
| `tremolite-tools` | Tool system — registry, execution, schema |
| `tremolite-plan` | Planning system |
| `tremolite-cron` | Scheduled tasks |
| `tremolite-delegation` | Task delegation to sub-agents |
| `tremolite-mcp` | Model Context Protocol integration |
| `tremolite-compress` | Session compression and context management |
| `tremolite-distiller` | Knowledge distillation |
| `tremolite-self-learner` | Autonomous learning pipeline |

## Links

- **[Artic Protocol](https://github.com/SpicySugar16/artic-protocol)** — The standard protocol Tremolite implements
- **[Artic Protocol SPEC](https://github.com/SpicySugar16/artic-protocol/blob/main/SPEC.md)** — Full protocol specification
- **[Tremolite Memory](https://github.com/SpicySugar16/tremolite-memory)** — Standalone 5-layer cached memory module
- **[Tremolite Reflection](https://github.com/SpicySugar16/tremolite-reflection)** — Metacognition & abstraction engine
- **[Discussion / Issues](https://github.com/SpicySugar16/tremolite/issues)** — Questions, bugs, and contributions welcome

## License

MIT

---

<div id="chinese-version"></div>

<details>
<summary><strong>🌐 中文版本</strong> (点击展开)</summary>

<br>

# 透闪石 Tremolite

> 基于 Rust 的模块化 AI 助手框架，遵循 [Artic Protocol](https://github.com/SpicySugar16/artic-protocol) 标准协议

<a href="#">English</a> · **中文**

![透闪石 Crate 架构](./docs/assets/crate-architecture.svg?t=1)

## 什么是透闪石

透闪石是一个用 Rust 编写的 **模块化 AI 助手框架**。它遵循 [Artic Protocol](https://github.com/SpicySugar16/artic-protocol)——一种智能体模块间的标准通讯协议——实现热插拔、跨语言的模块协作。

可以把它看作 Artic Protocol 的 **参考实现**：引擎通过模块声明的能力发现模块，按服务名路由请求，通过一条清晰的消息流管道编排工具执行、记忆、情绪、规划和 LLM 调用。

![模块消息流](./docs/assets/module-flow.svg?t=1)

## 架构

透闪石的 workspace 分为 **5 层**：

| 层 | 用途 | 包含 Crate |
|-----|------|-----------|
| **外部接口层** | 入口、协议适配、通道 | `tremolite-cli`, `tremolite-server`, `tremolite-dashboard`, `tremolite-plugin`, `tremolite-message`, `tremolite-channels` |
| **认知与智能层** | LLM 集成、情绪状态、记忆、注意力、学习 | `tremolite-llm`, `tremolite-emotion`, `tremolite-attention`, `tremolite-learn`, `tremolite-reflection`, `tremolite-distiller`, `tremolite-self-learner`, `tremolite-compress` |
| **核心引擎** | Phase 10 主循环、会话调度器、模块生命周期 | `tremolite-core` |
| **规划与工具层** | 规划、工具执行、委派、MCP、定时任务 | `tremolite-plan`, `tremolite-tools`, `tremolite-delegation`, `tremolite-mcp`, `tremolite-cron` |
| **基础设施层** | 配置、持久记忆、会话管理 | `tremolite-config`, `tremolite-memory`, `tremolite-session` |

## 快速开始

```bash
# 构建所有 crate
cargo build

# 运行测试
cargo test

# 启动 CLI
cargo run -p tremolite-cli

# 启动 HTTP 服务
cargo run -p tremolite-server
```

## Crate 概览

| Crate | 说明 |
|-------|------|
| `tremolite-core` | 核心引擎 — Phase 10 主循环、会话调度、模块注册 |
| `tremolite-plugin` | Plugin trait + 模块接口（遵循 Artic Protocol） |
| `tremolite-message` | 消息协议 — 信封、路由、序列化 |
| `tremolite-channels` | 多通道网关 — QQ Bot、NapCat、HTTP、WebSocket |
| `tremolite-session` | 会话生命周期管理 |
| `tremolite-config` | 配置解析与校验 |
| `tremolite-cli` | CLI 入口 — 交互式 Shell |
| `tremolite-server` | HTTP/WebSocket 服务（Axum） |
| `tremolite-dashboard` | 管理后台面板 |
| `tremolite-llm` | LLM 提供者抽象（OpenAI、DeepSeek、Ollama） |
| `tremolite-emotion` | 情绪状态引擎 |
| `tremolite-memory` | 五层缓存记忆（RAG） |
| `tremolite-attention` | 多尺度注意力机制 |
| `tremolite-learn` | 在线学习引擎 |
| `tremolite-reflection` | 自我反思与元认知 |
| `tremolite-tools` | 工具系统 — 注册、执行、schema |
| `tremolite-plan` | 计划书系统 |
| `tremolite-cron` | 定时任务 |
| `tremolite-delegation` | 子智能体任务委派 |
| `tremolite-mcp` | Model Context Protocol 集成 |
| `tremolite-compress` | 会话压缩与上下文管理 |
| `tremolite-distiller` | 知识蒸馏 |
| `tremolite-self-learner` | 自学习流水线 |

## 链接

- **[Artic Protocol](https://github.com/SpicySugar16/artic-protocol)** — 透闪石遵循的标准协议
- **[Artic Protocol SPEC](https://github.com/SpicySugar16/artic-protocol/blob/main/SPEC.md)** — 完整协议规范
- **[记忆模块](https://github.com/SpicySugar16/tremolite-memory)** — 独立五层缓存记忆模块
- **[反思模块](https://github.com/SpicySugar16/tremolite-reflection)** — 元认知与抽象引擎
- **[讨论 / Issue](https://github.com/SpicySugar16/tremolite/issues)** — 欢迎提问、汇报 bug、贡献代码

## 许可证

MIT

</details>
