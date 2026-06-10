---
name: 调试方法
category: tool
description: 系统性调试方法——二分定位、断言、crash dump分析、静默失败排查、网络问题调试
---

# 调试方法 (Debugging)

系统性调试方法——二分定位、断言、crash dump 分析、静默失败排查、网络问题调试。

## 方法论

1. **确认问题范围** — 新引入还是历史遗留？稳定复现还是偶发？
2. **二分定位** — 注释掉一半代码确认半区有无问题，四轮内定位到行
3. **加日志** — 怀疑的路径全部加上 `tracing::info!()`，看实际走了哪条路
4. **对比测试** — 改前/改后两套的差异就是根本原因

## 静默失败排查

- 查 `browser_console()` 获取浏览器端 JS 错误（前端）
- 查 `process(action='log')` 获取后台进程的 stdout/stderr
- 查 API 调用返回码、错误体、超时情况
- 断点调试：Python 用 `python -m pdb` + `import pdb; pdb.set_trace()`，JS 用 `browser_console(expression='debugger')`

## 工具选择

- 编译问题 → `cargo check`（比 build 快），`rust-analyzer` diagnostics
- 运行时问题 → `strace` / `ltrace` / `perf` / `valgrind`
- 网络问题 → `curl -v` 看 HTTP 层，`tcpdump -X` 看 TCP 层
- 前端问题 → Browser DevTools (F12)：Network/Console/Elements
