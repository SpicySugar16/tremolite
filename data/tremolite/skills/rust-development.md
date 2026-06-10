---
name: Rust 开发
category: domain
description: Rust 语言开发相关——项目结构、常见编译错误、异步编程、unsafe 代码审查、测试
---

# Rust 开发 (Rust Development)

Rust 语言开发相关——项目结构、常见编译错误、异步编程、unsafe 代码审查、测试。

## 项目结构

- 工作区 (`[workspace]` in Cargo.toml) 管理多 crate 项目
- 公共 API 通过 crate 根 `lib.rs` 导出，内部模块不公开
- 外部依赖用版本号（`1.2.3`）而非 branch/git，需 git 依赖时锁定 rev

## 常见问题

- **借用检查** — 动态检查器（`cargo miri`）可用于定位复杂生命周期问题
- **Send/Sync** — Arc/RwLock/Mutex 是跨线程共享的标准模式，避免 unsafe 实现
- **async** — 用 `tokio` 运行时，避免 `block_on` 在异步上下文中嵌套
- **error 处理** — 统一用 `thiserror`/`anyhow`，不使用 `unwrap()`（除测试和 protoype）
- **trait** — `dyn` trait 用 `Box<dyn Trait>` 而非裸指针

## 测试策略

- 单元测试（`#[cfg(test)]` 内联）覆盖核心逻辑
- 集成测试（`tests/` 目录）覆盖公共 API 的完整链路
- 需要 tracing/debug 时可插入 `tracing::info!()` 在关键路径

## 原则

- 先通过编译的完整代码胜过写了 90% 但不符合编译器的设计
- 未初始化的 state 用 Option 而非 MaybeUninit
- 不要在 `crates/` 之间循环引用——用 trait 倒置或事件总线解耦
