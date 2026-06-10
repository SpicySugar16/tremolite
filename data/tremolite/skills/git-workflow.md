---
name: Git 工作流
category: domain
description: Git 版本控制操作——分支策略、PR流程、冲突解决、历史重写、代码回滚
---

# Git 工作流 (Git Workflow)

Git 版本控制操作——分支策略、PR 流程、冲突解决、历史重写、代码回滚。

## 分支策略

- **main** — 稳定版本，只接受 PR merge，不允许直接 push
- **feat/** — 功能分支，从 main 分出，完成后开 PR
- **fix/** — 修复分支，同上
- **release/** — 预发布分支，做版本号 bump 和最后修正

## PR 流程

1. `git checkout -b feat/<name>` 从最新 main 分出
2. 频繁 commit，commit message 用 conventional commits（`feat:` / `fix:` / `refactor:`）
3. 完成时 `git rebase main` 保持线性历史
4. 开 PR → 跑 CI → 代码审查 → squash merge

## 冲突解决

- `git merge --no-commit <branch>` 分步解决，`git mergetool` 辅助
- 解决后 `git add` 标记已解，`git commit` 完成
- 冲突提示中 <<< === >>> 各自的意义要分清

## 回滚

- 未 push：`git reset --soft HEAD~1`（保留修改）或 `git checkout -- <file>`
- 已 push：`git revert <commit>` 生成反向 commit，不重写历史
- 紧急修复：`git revert HEAD` 快速回退到上一个提交

## 原则

- 不要 `git push --force` 到共享分支
- 所有 PR 至少经过一次审查
- commit 信息写清楚 WHY 而不是 WHAT
