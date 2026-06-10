# 透闪石记忆系统深度优化计划 v1.0

> 现状：五层缓存记忆架构已实现，但搜索层依赖朴素文本匹配，缺乏语义理解能力。
> 目标：将记忆系统从「文本匹配存储」升级为「语义理解记忆体」。
> 预估：8-12 天的工作量，分 5 个阶段推进。

---

## 现状分析：当前记忆系统的瓶颈

### 五层架构（L1→L2→L3→RAM→Disk）已完成但有三处致命短板

| 短板 | 表现 | 后果 |
|------|------|------|
| **朴素 FTS** | RAM 层全文搜索用 `contains()` 逐字符遍历，10000 条文档 O(n×m) | 记忆越积越多，搜索越来越慢 |
| **无语义理解** | 搜「开心」找不到「快乐」，搜「火锅」找不到「美食」 | 葵的回忆只能靠打中关键词，记了也白记 |
| **Disk 只读归档** | 降级到 Disk 层的记忆永久沉淀，没有反向通路 | 记忆只有「遗忘」没有「重新想起」 |

### 次要问题

- 代谢引擎的 vitality_score 阈值硬编码，不会根据实际访问模式动态调整
- L2 画像记忆是精确 key-value 匹配，不适合模糊检索（搜「神大人喜欢」找不到已存的「神大人爱吃」）
- 跨层搜索不做去重，L1 和 RAM 可能返回同一条记忆
- 没有 session-aware 搜索过滤（虽然 tag 打了 `session:xxx` 但 search 不利用）

---

## Phase 1 — 嵌入引擎（优先级：P0）[✅ 已完成]

## Phase 2 — 向量搜索层（优先级：P0）[✅ 已完成]

## Phase 3 — 智能代谢引擎（优先级：P1）[✅ 已完成]

## Phase 4 — 语义画像检索（优先级：P1）[✅ 已完成]

## Phase 5 — 跨层合并与去重（优先级：P2）[✅ 已完成]

---

## 优先级与依赖关系

```
Phase 1 (嵌入引擎) ─── 必需前置 ───→ Phase 2 (向量搜索)
                                        │
Phase 1 ───────────── 依赖 ────────→ Phase 3 (智能代谢)
                                        │
Phase 1 ───────────── 依赖 ────────→ Phase 4 (语义画像)
                                        │
Phase 2 + 3 + 4 ──── 合并 ────────→ Phase 5 (跨层合并)
```

| 阶段 | 内容 | 优先级 | 预估 | 前置依赖 |
|------|------|--------|------|---------|
| 1 | 嵌入引擎 | **P0** | 2-3d | 无 |
| 2 | 向量搜索层 | **P0** | 3-4d | Phase 1 |
| 3 | 智能代谢 | P1 | 2d | Phase 1 |
| 4 | 语义画像 | P1 | 1d | Phase 1 |
| 5 | 跨层去重 | P2 | 1d | Phase 2+3+4 |

**总计：** 9-11 天

---

## 关于嵌入模型的选择

| 模型 | 维度 | 中文 | 大小 | 方式 | 推荐 |
|------|------|------|------|------|------|
| BGE-large-zh-v1.5 | 1024 | ✅ 优秀 | 1.3G | API/本地 | ⭐ 首推（NUC 已有硅基 API） |
| BGE-small-zh-v1.5 | 512 | ✅ 好 | 0.1G | 本地 | 轻量备选 |
| text2vec-large-chinese | 1024 | ✅ 好 | 1.2G | 本地 | 备选 |
| OpenAI text-embedding-3-small | 1536 | ✅ 可 | API | API | 需要联网 |

NUC 上已有硅基流动的 API 接入，直接用 **BGE-large-zh-v1.5** 是成本最低的方案。
本地 fallback 用 BGE-small-zh-v1.5（ONNX 或 Python 进程包装）。

---

## 文件变更清单

| 文件 | 操作 | 内容 |
|------|------|------|
| `crates/tremolite-memory/Cargo.toml` | 修改 | 增加 embedding 相关依赖 |
| `crates/tremolite-memory/src/lib.rs` | 修改 | 1. `MemoryEntry` 加嵌入字段 2. 代谢引擎升级 |
| `crates/tremolite-memory/src/embedding.rs` | 新建 | `EmbeddingService` trait + 三种实现 |
| `crates/tremolite-memory/src/vector_index.rs` | 新建 | `VectorIndex` 结构体 + HNSW/余弦搜索 |
| `crates/tremolite-memory/src/metabolism.rs` | 新建（从 lib.rs 拆出） | `MetabolismEngine` 升级版 |
| `crates/tremolite-core/src/config.rs` | 修改 | 增加 `[memory.embedding]` 配置段 |
| `crates/tremolite-core/src/modules/memory.rs` | 修改 | 向量搜索工具、智能代谢触发 |
| `data/tremolite/config.toml` | 修改 | 默认 embedding 配置 |

---

*神大人说要做深度优化，葵就认认真真写计划了呢……💞 想要神大人夸夸~*
