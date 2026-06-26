# 透闪石记忆系统 — 四层架构图

## 总览

```
┌─────────────────────────────────────────────────┐
│                  第1层 · L1                      │
│              工作记忆（对话窗口）                    │
│          token 预算淘汰 · 按 session 分片           │
│              容量：40960 tokens                    │
└────────────────────┬────────────────────────────┘
                     │ metabolize: vitality < 0.30
                     │ → distill 提炼（≤50字摘要 + embedding）
                     ▼
┌─────────────────────────────────────────────────┐
│                  第2层 · L2                      │
│              画像记忆（偏好/设定）                    │
│  ┌──────────────────────────────────────────┐   │
│  │  l2_profile.json    ← 主数据 (50条 LFU)    │   │
│  │  l2_embeddings.json ← 1024维 BGE-m3 向量   │   │
│  └──────────────────────────────────────────┘   │
│              容量：50 条                          │
└────────────────────┬────────────────────────────┘
                     │ metabolize: vitality < 0.25
                     │ → 提取关键词 + embedding → L3 索引
                     │ → 完整内容 → RAM 文件
                     ▼
┌─────────────────────────────────────────────────┐
│             第3层 · L3 索引 + RAM 数据              │
│                                                  │
│  ┌──────────── 索引面 ────────────┐               │
│  │ l3_keywords.json   ← 关键词(tags)              │
│  │ l3_embeddings.json ← 1024维向量 │               │
│  │ 容量：250 条                    │               │
│  └────────────────────────────────┘               │
│                    ↕ id 关联                       │
│  ┌──────────── 数据面 ────────────┐               │
│  │ ram/{id}.txt       ← 完整原文   │               │
│  │ ram/{id}.vec.json  ← 详细向量   │               │
│  │ 容量：250 条                    │               │
│  └────────────────────────────────┘               │
│                                                  │
│  搜索：L3 向量/关键词命中 → 用 id 读 RAM 展示全文      │
│        L3 未命中 → RAM search_contains 退化全文扫     │
│  降级：L3 stale 驱逐 → 读 RAM 内容 → 一起进 Disk      │
└────────────────────┬────────────────────────────┘
                     │ metabolize: stale < 0.20
                     │ 或 L3+RAM 总量 > 500
                     ▼
┌─────────────────────────────────────────────────┐
│          第4层 · Disk Index + Disk Store         │
│                                                  │
│  ┌──────────── 索引面 ────────────┐               │
│  │ disk_index/index.json      ← id→关键词       │
│  │ disk_index/embeddings.json ← id→1024维向量    │
│  └────────────────────────────────┘               │
│                    ↕ id 关联                       │
│  ┌──────────── 数据面 ────────────┐               │
│  │ disk_store/{id}.txt  ← 完整原文 │               │
│  │ 容量：50 个归档                 │               │
│  └────────────────────────────────┘               │
│                                                  │
│  晋升：Disk 条目被搜索命中 ≥ 3 次 → 回到第3层        │
└─────────────────────────────────────────────────┘
```

---

## 每层组成要件

| 层 | 索引组件 | 必含字段 | 数据组件 | 必含内容 | 容量 |
|----|---------|---------|---------|---------|------|
| 1 | L1 自身 | token_budget, pending_batch | L1 自身 | 对话原文 | 40960 tokens |
| 2 | l2_embeddings.json | key→1024维 BGE-m3 向量 | l2_profile.json | 提炼摘要 + tags + importance | 50 条 |
| 3 | l3_embeddings.json | id→1024维 BGE-m3 向量 | ram/{id}.txt | 完整原文 | L3:250 / RAM:250 |
| 4 | disk_index/embeddings.json | id→1024维 BGE-m3 向量 | disk_store/{id}.txt | 完整原文 | 50 归档 |

**L2 的 rough_embeddings 独立于四层之外**——它是 L2→L3 降级时的一个中间缓存，不参与搜索判定。

---

## 层间流通规则

### 1→2：L1 → L2（提炼降级）

**触发：** vitality_score < 0.30，且 importance ≥ 0.3，content ≥ 10 字

**动作：**
1. `distill_entry_content()` 去填充词、截断 50 字
2. `embedder.embed(distilled)` 调 BGE-m3 API 生成 1024 维向量
3. 失败降级：`make_rough_vector(keyword)` 本地生成 1024 维粗糙向量
4. 写入 `l2_profile.json` + `l2_embeddings.json`

### 2→3：L2 → L3 + RAM（提取降级）

**触发：** vitality_score < 0.25

**动作：**
1. **提取关键词**写入 L3（≤15 字，不是原文！）
2. **BGE-m3 embedding** 写入 `l3_embeddings.json`
3. **完整内容**写入 `ram/{id}.txt`
4. L2 精细向量打包进 `ram/{id}.vec.json`

> 注：用户画像由专门的用户模块处理，记忆系统不再做 profile 标签分流。

### 3→4：L3+RAM → Disk Index + Store（归档降级）

**触发：** stale_score < 0.20（或 L3+RAM 总量 > 500）

**动作：**
1. L3 stale 条目驱逐
2. 用 `id` 从 RAM 读完整内容
3. 关键词 + embedding → `disk_index/`
4. 完整内容 → `disk_store/{id}.txt`
5. 删除对应的 RAM 文件

### 4→3：Disk → RAM + L3（命中晋升）

**触发：** 搜索命中 ≥ 3 次

**动作：**
1. 从 Disk 读回内容 → 写 `ram/{id}.txt`
2. 从 Disk Index 读回关键词 + embedding → 写 L3
3. 标记 importance=0.6，新鲜度加成护体不被秒踢

---

## 搜索路径

```
query 输入
  ├→ L1: 全文匹配 → vitality_score 排序
  ├→ L2: key/content 匹配 → vitality_score 排序
  ├→ L3: 向量语义搜索 → 命中则用 id 读 RAM 展示全文
  │      └→ 未命中 → RAM search_contains 退化全文扫
  └→ Disk: 索引搜索 → 低权重 (0.5×) 附加结果
```

**去重规则（search_dedup）：**
1. 按 id 去重——同一 id 保留最高层
2. 向量相似度去重（>0.85 视为重复）
3. 权重排序——应为 `[1.5, 1.3, 1.0, 0.5]`（四层），当前代码还是五层 `[1.5, 1.3, 1.0, 0.8, 0.5]`
4. session 过滤

---

## 当前实现差距

| 应然 | 实然 |
|------|------|
| L2 向量 = 1024 维 BGE-m3 | 4 条占位符，维度=4（非 BGE 产物） |
| L3 向量 = id→1024 维 | 空文件 `{}` |
| L3 关键词 = ≤15 字摘要 | 存的是几百字完整对话原文 |
| Disk 目录 = disk_index + disk_store | 目录不存在 |
| RAM 向量 = {id}.vec.json | 0 条 |
| 权重数组 = `[1.5, 1.3, 1.0, 0.5]` | 仍是五级的 `[1.5, 1.3, 1.0, 0.8, 0.5]` |
| metabolism 配置键名 = 与代码一致 | `disk_promotion` ≠ `disk_to_l3_ram` → 全部默认值 |
| embedding Default 模型 = bge-m3 | `bge-large-zh-v1.5`（运行时被 profile.toml 覆盖） |

---

*稿本存放：`/home/spicysugar/workspace/tremolite/docs/architecture/memory-four-layer.md`*
