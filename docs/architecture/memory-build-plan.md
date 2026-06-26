# 透闪石记忆模块 — 重构计划书

> 基于四层架构图（`docs/architecture/memory-four-layer.md`）
> 目标：从当前的空壳向量库 + 残留五层代码，重构为完整的四层记忆系统

---

## 总体路线

```
Phase 0  清理遗产         切除 ProfileCache、修复默认值、统一权重
Phase 1  接通嵌入管道      让 BGE-m3 向量真正流入各层
Phase 2  修 L2 向量库      l2_embeddings.json 存 1024 维向量
Phase 3  修 L3 向量库      l3_embeddings.json 存 id→1024 维向量
Phase 4  修 L3 关键词      摘要替代全文，≤15 字
Phase 5  建 RAM 向量       ram/{id}.vec.json 存详细向量
Phase 6  建 Disk 层        disk_index/ + disk_store/ 完整落地
Phase 7  修复 metabolism   配置键名对齐、自定义权重生效
Phase 8  集成测试          端到端：remember → metabolize → search_dedup
```

---

## Phase 0：清理遗产

### 0.1 删除 ProfileCache
- **文件：** `lib.rs` ProfileCache 结构体、`profile_cache` 字段、`maintain()` 调用
- **文件：** `lib.rs` `is_profile_related()` 函数
- **文件：** metabolism 中 profile 标签分流逻辑（lib.rs:2089-2095）
- 用户画像由专门的用户模块处理，记忆系统不再做画像分流

### 0.2 修复 search_dedup 权重数组
- **位置：** lib.rs:1996
- **改前：** `let layer_weights: [f64; 5] = [1.5, 1.3, 1.0, 0.8, 0.5];`
- **改后：** `let layer_weights: [f64; 4] = [1.5, 1.3, 1.0, 0.5];`（L3 和 RAM 合并为一个权重）
- 同步修改 `level_order` 数组和所有依赖 5 级索引的去重逻辑

### 0.3 修复 embedding Default 模型
- **位置：** embedding.rs:32
- **改前：** `model: "BAAI/bge-large-zh-v1.5".into()`
- **改后：** `model: "BAAI/bge-m3".into()`
- 与 `profiles/aoi/profile.toml` 保持一致

### 0.4 清理 L3 关键词残留
- 清空当前 `l3_keywords.json` 中存的全文字段（4 条），这些不是关键词
- 为 Phase 4 的正确关键词存储腾出干净起点

---

## Phase 1：接通嵌入管道

### 目标
确认 `EmbeddingService` 在 `MemoryManager` 中正确初始化，所有层边界都能调用 BGE-m3 API。

### 检查项
1. `MemoryManager.embedder` 字段是否存在且被正确注入
2. 当前 `Option<Box<dyn EmbeddingService>>`（lib.rs:1280 附近）是否在 `new()` 时被 Some 填充
3. L1→L2 提炼时 `self.embedder.as_ref().and_then(|e| e.embed(&distilled).ok())` 是否真的调到了 API

### 修复方向
如果 embedder 为 None（当前症状：L2 向量只有 4 维本地占位符，从未调 API），则：
- 在 `MemoryManager::new()` 中接收 `EmbeddingConfig` 并构造 `SiliconFlowEmbedder`
- 从 `profiles/aoi/profile.toml` 读取 api_key、model、api_url

---

## Phase 2：修 L2 向量库

### 目标
`l2_embeddings.json` 中每条记录 = key → 1024 维 BGE-m3 向量

### 改动
1. L1→L2 提炼时：`embedder.embed(distilled)` → 1024 维 Vec<f32>
2. `l2.set_with_embedding(key, content, tags, importance, emb)` 正确写入 `embedding_store`
3. flush 时 `emb_dirty=true` → 序列化到 `l2_embeddings.json`
4. 替换当前 4 条 4 维占位符

### 验证
```bash
python3 -c "import json; d=json.load(open('l2_embeddings.json')); print(len(d), '条, dim=', len(list(d.values())[0]))"
# 应输出: N 条, dim= 1024
```

---

## Phase 3：修 L3 向量库

### 目标
`l3_embeddings.json` 中每条记录 = id → 1024 维 BGE-m3 向量

### 改动
1. L2→L3 降级时（lib.rs:2099-2116）：
   - 关键词 embedding 走 BGE-m3 API（不走 rough fallback）
   - `l3.add(IndexEntry { embedding: Some(emb), ... })` — 确保 embedding 是 Some
2. L3 flush 时（lib.rs:578-604）：
   - `l3_embeddings.json` 写入完整的 `HashMap<u64, Vec<f32>>`
3. 替换当前空文件 `{}`

### 验证
```bash
python3 -c "import json; d=json.load(open('l3_embeddings.json')); print(len(d), '条'); [print(f'  id={k} dim={len(v)}') for k,v in list(d.items())[:3]]"
# 应输出真实 id 和 1024 维
```

---

## Phase 4：修 L3 关键词

### 目标
L3 索引存的是 ≤15 字关键词摘要，不是几百字对话原文。

### 改动
1. 在 `L2ProfileMemory::evict_demoted` 或 MemoryManager 的 L2→L3 降级路径中：
   - `keywords = entry.content.chars().take(15).collect()` 保持不变
   - **但当前 source 是 L2 的 entry.content，而 L2 存的已经是 distilled 的 ≤50 字摘要**
   - 如果 L2 内容仍是长文本，需在降级时再截一次
2. 禁止将 `entry.content` 原样存入 L3 keywords
3. 搜索时 L3 命中 → 用 id 读 RAM 展示全文（搜索路径已正确，但需确认 RAM 中有对应全文）

### 验证
```bash
python3 -c "import json; d=json.load(open('l3_keywords.json')); [print(f'  id={k} len={len(v)} text={v[:60]}') for k,v in list(d.items())[:5]]"
# 每条长度应 ≤15
```

---

## Phase 5：建 RAM 向量

### 目标
`ram/{id}.vec.json` 存储 L2→L3 降级时从 L2 带来的精细向量。

### 改动
1. L2→L3 降级时（lib.rs:2105-2107）：
   - 如果 `l2_detailed` 存在 → `self.ram.store_vector(entry.id, detailed)` 
   - 当前代码**已经有这行**，但实际没有生成 `.vec.json` 文件
2. 确认 `RamFileStore::store_vector()` 的路径正确，数据目录有写入权限
3. 确认 `RamFileStore::remove()` 同步清理 `.vec.json`

### 验证
```bash
ls ram/*.vec.json | wc -l
# 应与 L2→L3 降级条目数一致
```

---

## Phase 6：建 Disk 层

### 目标
Disk 层完整落地：`disk_index/` + `disk_store/`

### 改动
1. 确保 `DiskColdArchive::new()` 创建 `disk_index/` 和 `disk_store/` 目录
2. L3+RAM→Disk 降级时：
   - `store_entry(id, keyword, content, created, embedding)` 写 `disk_store/{id}.txt`
   - 同步写 `disk_index/index.json` 和 `disk_index/embeddings.json`
3. search 时 Disk 返回结果正确加权
4. Disk→RAM 晋升路径验证

### 验证
```bash
ls archives/disk_store/*.txt | wc -l
ls archives/disk_index/index.json && python3 -c "import json; print(len(json.load(open('archives/disk_index/index.json'))))"
```

---

## Phase 7：修复 metabolism 配置

### 问题
`metabolism.toml` 用了 `disk_promotion` 作为 section 名，代码中在某个路径期望 `disk_to_l3_ram`。

### 改动
1. 找到 `MemoryManager` 或 `MetabolismEngine` 中 `tier_config(idx)` 解析配置的代码
2. 统一键名——要么改代码、要么改 toml
3. 验证：启动时不再出现 `WARN metabolism config parse failed`
4. 确认自定义权重（L3+RAM→Disk 的 `recency:0.2/frequency:0.2/importance:0.6`）生效

---

## Phase 8：集成测试

### 测试用例
```
1. 写入一条 remember("神大人不喜欢吃海鲜")
2. 等待 metabolize 触发（或手动触发）
3. 验证 L2 有提炼后的摘要 + 1024 维向量
4. 再次 metabolize → 验证 L3 有关键词 + 向量，RAM 有原文
5. search_dedup("神大人不喜欢什么") → 验证返回正确结果
6. search_dedup("海鲜") → 向量语义搜索能跨关键词命中
```

### 端到端数据流验证
```
remember() → L1 push → metabolize L1→L2 → L2 有 1024 维向量
           → metabolize L2→L3+RAM → L3 有 ≤15 字关键词 + 1024 维向量
                                  → RAM 有 {id}.txt 全文 + {id}.vec.json
           → metabolize L3+RAM→Disk → Disk 有索引+向量+内容
           → search_dedup(query) → 四层权重正确、去重正确
```

---

## 实现优先级

| 优先级 | Phase | 理由 |
|--------|-------|------|
| 🔴 P0 | 0.2 权重修复 | 搜索排序直接受影响，一行改动 |
| 🔴 P0 | 0.3 Default 模型 | 哪天不传配置就退回去 |
| 🔴 P0 | 7 metabolism 配置 | 自定义权重全白配了 |
| 🟡 P1 | 0.1 删 ProfileCache | 死代码，用户模块已接手 |
| 🟡 P1 | 1 嵌入管道 | 一切向量的前提 |
| 🟡 P1 | 2 L2 向量 | 画像语义搜索的前提 |
| 🟢 P2 | 3 L3 向量 | 历史语义搜索的前提 |
| 🟢 P2 | 4 L3 关键词 | 索引用途正确性 |
| 🟢 P2 | 5 RAM 向量 | L3→L2 晋升时向量回传 |
| 🔵 P3 | 6 Disk 层 | 冷归档，可后置 |
| 🔵 P3 | 8 集成测试 | 验证闭环 |

---

*计划书存放：`docs/architecture/memory-build-plan.md`*
*关联文档：`docs/architecture/memory-four-layer.md`*
