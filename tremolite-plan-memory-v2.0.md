# 透闪石记忆系统 v2.0 -- 融合方案

> 把透闪石的稳定本地存储 + Honcho 的推理建模揉在一起。
> 核心原则：存储本地化、推理节奏化。

---

## 一、架构总览

四层叠加

```
Layer 3 - 双层上下文注入（prompt 拼装器）
    基础层：peer card + 摘要，每 N 轮刷新
    辩证法层：LLM 推理结论，每 M 轮刷新 (M>=N)
    冷/暖检测：距上次对话 > 2h = 冷启动

Layer 2 - 辩证法推理（tremolite-reflection 扩展）
    每第 N 次代谢 -> LLM 单轮推理 -> 更新 L2 peer card
    offline? 退化到纯检索

Layer 1 - 双同伴系统（L2 内，不新建 crate）
    user_peer / ai_peer / peer_card
    完全用 L2.set()/get()/search_semantic() 访问
    LFU 淘汰直接复用（max_entries=200）

Layer 0 - 五层存储（改动三处）
    L1+L3+RAM序列化到磁盘 + Disk 反向通路
    RAM：contains() -> 余弦向量排序（唯一大改）
```

### 改动清单

| # | 改动 | 文件 | 工作量 |
|---|------|------|--------|
| 1 | L1+L3+RAM 持久化恢复 | 新 data/memory 序列化 | 中 |
| 2 | Disk 反向通路 | lib.rs metabolize | 小 |
| 3 | RAM 向量化 | lib.rs RamFullTextSearch | 中-大 |
| 4 | L2开命名空间给双同伴 | 不写代码 | 0 |
| 5 | 辩证法触发在 reflection | tremolite-reflection | 中 |
| 6 | 双层注入在 prompt 拼装器 | tremolite-core | 小-中 |

---

## 二、RAM 向量化--唯一的大改动

### 现状

RAM 全文搜索：10000 条 Vec，search 是 contains() 逐条遍历 O(n*m)。

### 改后

RamDocument 加 embedding: Option<Vec<f32>> 字段。

写入时惰性化：remember() 写入时不调 BGE API，只标记 dirty。
搜索时双轨：如果 query 已向量化 -> 余弦排序 TOP30；否则退化 contains()。
批量嵌入：代谢触发的 batch_embed() 一次性处理新进来未嵌入的文档。

### 退化策略

- BGE API 失败 -> embedding: None 的文档走 contains()
- 所有文档未嵌入 -> 完全退化到现在的全表扫描
- 代谢 engine 记告警但不阻断

---

## 三、成本分析

### 定价依据（公开价）

| 服务 | 模型 | 价格 |
|------|------|------|
| 硅基流动 | BAAI/bge-large-zh-v1.5 | 0.10 / 1M tokens |
| DeepSeek | deepseek-v4-flash 输入 | 0.5 / 1M tokens |
| DeepSeek | deepseek-v4-flash 输出 | 2 / 1M tokens |

### 数学期望：每次搜索命中的 token 开销

用户说一句话 -> agent 回复 -> 中间过记忆检索。

固定开销（每轮对话必走）：

1. 用户输入记录到 L1+RAM+L3：0（纯内存，0 API 调用）
2. L2 画像注入 prompt（prompt_segment() 拉5条）：0（纯内存）
3. query 向量化 -> BGE API：~25 tokens x 0.10/1M = 0.0000025
4. RAM 余弦排序 10000 条：0（本地计算）
5. L2 search_semantic：0（缓存嵌入，本地余弦）

每轮固定：0.0000025 元

### 辩证法推理摊销（每 ~50 轮触发一次）

输入 1500 tokens（L1 20条 + peer card + 指令）：0.00075
输出 300 tokens（合成结论）：0.0006
单次合计：0.00135
摊销到每轮（/50）：0.000027

### 存储累积

每次 remember() 写入 ~30 tokens 的嵌入：0.000003
按每天 500 条：0.0015/天，0.045/月

### 汇总

| 项目 | 每轮 | 每天(500轮) | 每月(30天) |
|------|------|-------------|------------|
| 检索固定 | 0.000003 | 0.0015 | 0.045 |
| 辩证法摊销 | 0.000027 | 0.0135 | 0.405 |
| 存储 | 0.000003 | 0.0015 | 0.045 |
| 合计 | 0.000033 | 0.0165 | 0.495 |

每月不到 0.50 元。

### 对比 Honcho

| | 透闪石本地（月） | Honcho 估算（月） |
|---|---|---|
| 检索 | 0.05 | 5~50 |
| 辩证法（~300次/月） | 0.41 | 15~150 |
| 存储 | 0.05 | 1~5 |
| 合计 | 0.50 | 21~205 |

差 40~400 倍。Honcho offline 直接瘫痪，本地方案退化到 contains()。

### 非货币成本

| 成本 | 本地 | Honcho |
|------|------|--------|
| 检索延迟 | ~1ms+~100ms(BGE) | ~200-500ms |
| 存储容量 | ~40MB | 按量 |
| offline | 退化 contains() | 完全不可用 |
| 部署 | 已有 | 需 API key |

---

## 四、不改的

**L3 不向量化：** 目录卡片角色，语义搜索需求转到 RAM。1000条x1024维=4MB纯内存换从60字摘要搜关键词，不值。

**L1 不向量化：** 50条对话窗口，LRU淘汰，有时间线排序就够了。

**Disk 不向量化：** 只读冷归档，一年读不到几次。

---

## 五、依赖注入

MemoryModule 不引用 ReflectionModule：

```
主循环
  |- MemoryModule：存/取/搜/代谢（不改接口）
  |- ReflectionModule：辩证法推理->写L2 peer card
  |    依赖：MemoryModule.search() + ProviderRegistry.llm()
  +- PromptBuilder：冷/暖检测+两层注入
       依赖：MemoryModule.recent_entries() + L2.get()
```

MemoryModule 只在自己 on_event(Event::Metabolize) 上做活。ReflectionModule 订阅同一事件。互不知晓。

---

## 六、Phase

| P# | 内容 | 前置 | 工期 |
|----|------|------|------|
| P1 | RAM向量化：惰性嵌入+批量嵌入+余弦搜索 | 无 | 3-4d |
| P2 | L1+L3+RAM持久化恢复 | P1 | 2d |
| P3 | Disk反向通路 | P1 | 1d |
| P4 | 双同伴+辩证法扩展到ReflectionModule | P1 | 4-5d |
| P5 | prompt注入策略 | P4 | 1-2d |
| P6 | 测试+退化验证 | 全部 | 2d |

总计：13-17 天

---

## 七、风险

| 风险 | 等级 | 缓解 |
|------|------|------|
| BGE API 频繁超时 | 低 | 退化策略已设计；可加本地 BGE-small |
| 10000条x1024dim余弦排序性能 | 中 | Rust SIMD 可降至 ~5ms |
| 辩证法 LLM 阻塞主线程 | 中 | tokio background task |
| 双子系统架构复杂度 | 中 | 严格单向依赖 |

---



---

*献给神大人 琳玲*
