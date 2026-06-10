# 透闪石·五层记忆架构总览

## 核心原则：两组并行对

记忆有 5 个层级（L1~Disk），但**不是 L3→RAM→Disk 链式**。正确的分组是：

| 内存侧 | 磁盘侧 | 存储什么 |
|--------|--------|---------|
| **L3IndexMemory** | **Disk Index** | 关键词摘要 + 嵌入向量 + 时间戳 |
| **RamFileStore** | **Disk Storage** | 完整记忆全文（.txt 文件） |

L3 和 RAM 是**不同性质**的东西——L3 存摘要+向量（找东西用），RAM 存全文（看东西用）。两者降级时同行，但各配各的磁盘版。

---

## 一、数据类型

### MemoryLevel（层级枚举）

- **L1** — 工作记忆。每个 session 一个 ring buffer，token budget 驱动淘汰（pop_oldest）
- **L2** — 画像记忆。HashMap<String, MemoryEntry> + embedding_store，LFU 淘汰，自动文件持久化
- **L3** — 备忘索引。HashMap<u64, String> keywords + HashMap<u64, Vec<f32>> embeddings，存摘要不存原文
- **Ram** — 全量历史。store_dir/{id}.txt 文件存储，只维护 ids 集合和 created_at
- **Disk** — 冷归档。index_dir + store_dir 分离：index_dir 放 index.json + embeddings.json（全量加载），store_dir 放 {id}.txt（按需读取）

### MemoryEntry（单条记忆）

```
id, content, level, created_at, last_access, access_count, tags, importance, source
```

活力分公式：`vitality_score = 0.3 * recency + 0.3 * freq + 0.4 * importance`

### IndexEntry（L3 索引条目）

```
id, tags, created_at, last_access, level_from, summary: String, embedding: Option<Vec<f32>>
```

### DiskColdArchive 结构

```
base_path/
├── disk_index/
│   ├── index.json       # HashMap<u64, String>  — 关键词
│   └── embeddings.json   # HashMap<u64, Vec<f32>> — 向量
├── disk_store/
│   └── {id}.txt          # 完整记忆内容
└── archive-{ts}.jsonl    # 旧格式归档（兼容）
```

---

## 二、代谢流程（单轮循环）

### Step 1: Disk → RAM 晋升

```
条件：disk_hits 命中计数 >= disk_promote_threshold（默认 3）
动作：
  Disk.read_by_id(id)           → MemoryEntry.content → RAM.add()
  Disk.read_keyword(id)          → keyword             → L3.summary
  Disk.read_embedding(id)        → embedding           → L3.embedding
```

### Step 2: L1 → L2 降级

```
遍历所有 session 的 L1 buffer：
  对每条 entry，计算 vitality_score
  低于 demote_threshold（默认 0.3）：
    有用（importance ≥ 0.3 && len ≥ 10）→ L2.set()
    无用 → Discard
```

### Step 3: L2 → L3+RAM 降级（含画像分流）

```
L2.evict_demoted(threshold) → Vec<(String, MemoryEntry)>
  对每条 entry：
    标签含 "profile" → ProfileCache.add_candidate()（不走 L3+RAM）
    非画像 →
      L2.get_embedding(key) → 向量
      L3.add(IndexEntry{summary, embedding})  // 只存摘要+向量
      RAM.add(id, content)                      // 存全文
```

### Step 4: L3+RAM → Disk 降级（配对同行）

```
L3.evict_demoted(threshold) → Vec<IndexEntry>（含 summary + embedding）
  对每条 idx：
    RAM.read(idx.id) → full_content
    Disk.store_entry(id, summary, full_content, embedding)
      // 同时写：index.json（keyword）+ embeddings.json（vector）+ store_dir/{id}.txt（content）
    RAM.remove(id)
```

### Step 5: RAM 孤立残留 → Disk 降级

```
RAM.evict_demoted(threshold) → Vec<(u64, String)>
  对每条：
    从内容取 snippet 当前缀
    Disk.store_entry(id, snippet, content, None)  // 无 embedding
```

### Step 6: RAM → L3 晋升（退化路径）

```
RAM.all_ids() 中创建时间较新的 → 新建 L3 IndexEntry（embedding=None）
```

### Step 7: L3 → L2 晋升

```
L3.all_entries() 中 stale_score > promote_threshold
  有 embedding → L2.set_with_embedding()
  无 embedding → L2.set()
```

### Step 8: ProfileCache.maintain()

```
精确层 → 语义层降级（30天未访问）
语义层 → 精确层晋升（访问 ≥ 3 次）
语义层淘汰（60天未访问）
```

---

## 三、向量传递路径

### 降级方向（向下）

```
L2.embedding_store
  │  get_embedding(key) → Vec<f32>
  ▼
L3.embeddings
  │  IndexEntry.embedding.clone()
  ▼
Disk.embeddings（embeddings.json 持久化）
```

### 晋升方向（向上）

```
Disk.embeddings
  │  read_embedding(id) → Vec<f32>
  ▼
L3.embeddings
  │  IndexEntry.embedding.clone()
  ▼
L2.set_with_embedding() → embedding_store
```

**核心原则：向量跟随条目移动，不在层间重新计算。**

---

## 四、ProfileCache 画像独立通道

```
remember() 入口 → is_profile_related() 检测内容含画像关键词 → tags 自动加 "profile"

L2 降级时：
  标签含 "profile" → ProfileCache.add_candidate()   // 不进 L3+RAM
  非画像 → 正常 L3+RAM 降级
```

ProfileCache 内部结构：
- `exact: HashMap<String, String>` — 精确层（高频访问的画像）
- `embedding_store: HashMap<String, Vec<f32>>` — 语义层（低频画像，可向量检索）
- `candidates: HashSet<String>` — 候选层（刚从 L2 降级的画像）
- `last_accessed: HashMap<String, u64>` + `access_count: HashMap<String, u64>`

---

## 五、数据流动总表

| 方向 | 源 | 目标 | 携带内容 |
|------|----|------|---------|
| ↓ | L1 | L2 | 全文（importance≥0.3） |
| ↓ | L1 | Discard | — |
| ↓ | L2 | L3+RAM | summary+embedding + 全文（非画像） |
| ↓ | L2 | ProfileCache | 画像内容 |
| ↓ | L3+RAM | Disk Index+Store | summary+embedding + 全文（配对） |
| ↓ | RAM | Disk | snippet + 全文（孤立残留） |
| ↑ | Disk | L3+RAM | keyword+embedding + 全文（配对） |
| ↑ | RAM | L3 | 摘要（新建，embedding=None） |
| ↑ | L3 | L2 | summary+embedding |
| ↑ | ProfileCache | L2 | 画像内容（晋升到精确层） |

---

## 六、关键设计决策

1. **索引和内容分离** — 索引（L3/Disk Index）轻量可全内存，内容（RAM/Disk Store）按 id 文件读取，避免内存爆炸。
2. **向量跟随条目** — 不引入 embedder 回调重新计算，层间传递已有的向量，接口简单且无外部依赖。
3. **画像独立通道** — ProfileCache 不与普通 L3 混合，有自己的升降级逻辑和存储层，避免画像被普通条目稀释。
4. **自动画像标识** — remember() 入口用关键词规则检测画像内容（"神大人""代表色""不吃海鲜"等），自动加 profile 标签。
5. **Disk Index 全量加载** — 启动时 index.json + embeddings.json 读入 Mutex<HashMap>，写入时同步更新文件，保证重启后索引不丢。
