---
name: 文件操作
category: tool
description: 文件系统操作的正确方式——文件读写、搜索、编辑、权限管理、编码处理
---

# 文件操作 (File Operations)

文件系统操作的正确方式——文件读写、搜索、编辑、权限管理、编码处理。

## 文件类型工作流

- **读取** — 已知路径用 `read_file`，查找用 `search_files(target='content')` 或 `search_files(target='files')`
- **写入** — 用小文件直接写，大文件分块。绝对不用 echo/cat heredoc
- **编辑** — 用 `patch`（find+replace），不用 sed/awk。fuzzy matching 处理缩进/空白差异
- **搜索** — 内容搜索用 `search_files`（ripgrep 引擎），文件搜索用 `search_files(target='files')` 替代 ls

## 注意事项

- 备份重要配置文件再修改
- 权限问题（Permission denied）先检查 owner/group，再 `stat`
- 编码问题优先尝试 UTF-8，不行再试 GBK
- 大文件用 offset/limit 分页读取，不一次性读完
