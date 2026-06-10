---
name: MCP 工具
category: tool
description: MCP (Model Context Protocol) 工具的使用和调试——工具发现、调用、故障处理
---

# MCP 工具 (MCP Tool Usage)

MCP (Model Context Protocol) 工具的使用和调试——工具发现、调用、故障处理。

## 使用方法

1. MCP 服务器在启动时自动注册工具到全局工具列表
2. 工具命名规则：`<服务名>.<工具名>`（如 `filesystem.read_file`）
3. 用 `list_tools` 查看所有可用工具，含 MCP 注册的工具
4. 调用方式和普通工具一致

## 故障排查

- MCP 工具返回空 → 检查 MCP server 是否存活
- MCP 工具超时 → `config.toml` 中增大 `timeout_secs`
- 工具注册失败 → 查看启动日志 `Starting MCP server <name>...`
- 同名工具冲突 → MCP 客户端自动用 `<服务名>.<工具名>` 前缀去重

## 限制

- MCP 协议不支持流式响应
- 部分 MCP server 的 resources/prompts 返回可能为空（已实现但不一定可用）
