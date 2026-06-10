# Session 模块改造计划书

## 一、现状

```
tremolite-session ── 薄登记簿，只有 id + last_active
  SessionManager { sessions: HashMap<String, SessionState>, ttl }
  → 不注册为 Module，不接收事件，不贡献 prompt

tremolite-memory ── 五层全管，但 L1 是全局的
  MemoryManager {
    l1: L1WorkingMemory { buffer: VecDeque<MemoryEntry>, capacity: 50 }
    l2/l3/ram/disk ...
  }
  → recent_entries(20) 拉全局 L1，所有 session 混在一起
  → recent_entries_by_session() 做 tag 过滤，杯水车薪

tremolite-core ── engine 跑主循环
  process_with_llm() 调 mm.recent_entries(20) → 全局历史，无 session 感知
```

三个问题：
1. **L1 不分片**：多 session 消息混在一条队列里，capacity 50 被非活跃 session 占满
2. **跨 session 互通不存在**：A session 不知道 B session 在聊什么，开天窗
3. **session 模块是空壳**：空有状态登记，不 orchestrate

## 二、目标架构

```
                     ┌──────────────────────┐
                     │    tremolite-session  │
                     │  (SessionManager +    │
                     │   CrossSessionRing)   │
                     │                      │
                     │  OnMessage→touch+push │
                     │  prompt_segment→ring │
                     └──────┬───────────────┘
                            │
                ┌───────────┴──────────────────────────┐
                │   tremolite-memory                    │
                │                                       │
                │  L1：分片 session 原文                 │
                │   HashMap<session_id, L1Buffer>       │
                │   token 预算驱动，只读不写                │
                │   攒够 N 条 → 批量提炼 → 进 L2          │
                │                                       │
                │  L2：信息池（提炼条目 + 用户画像）       │
                │   晋升上限 L2，不可解压回原文             │
                │   代谢：低使用率 → 精简索引进 L3         │
                │         完整条目进 RAM                  │
                │                                       │
                │  L3+RAM：关键词索引 + 编号文件           │
                │   L3: { 编号, 关键词 }                 │
                │   RAM: { 编号.txt → 完整条目 }         │
                │   关键词→编号→RAM 等於图书馆索引→书      │
                │                                       │
                │  Disk：冷归档                           │
                │   镜像 L3+RAM：索引目录 + 文件仓库       │
                │   编号一对一绑定                       │
                │                                       │
                │  快速库 ProfileCache                   │
                │   平行于 L3，两层：                    │
                │   ① 精确命中 QQ号/名字 → 画像          │
                │   ② 向量退化兜底 → 画像                 │
                │   画像也带编号，可追溯到提炼来源          │
                └──────────────────────────────────────┘
                     │
                ┌────┴──────────────────────────┐
                │  tremolite-core / reflection   │
                │  反思流程：                     │
                │  ① 收到消息                     │
                │  ② 并行：读 L1 原文 + 加载画像    │
                │  ③ 读 L2 画像 + 关键词          │
                │  ④ 三路综合 → 生成建议           │
                │  ⑤ 建议 + 消息 → LLM → 回复     │
                │  ⑥ 用户回应后验证 → 更新画像      │
                └────────────────────────────────┘
```

## 三、模块边界

### SessionModule（tremolite-session，新注册为 Module）

**管的：**
- Session 生命周期：`get_or_create(sid)`→`touch(sid)`→`reap_expired()`→ 通知 memory 清理 L1 分片
- 跨 session 通知环：`CrossSessionRing { buffer: VecDeque<Note>, cap: 50 }`，OnMessage 压入**提炼后的**短观察
- 提炼规则（纯文本规则，不调 LLM）：
  1. 截断至 50 字（比原始 80 字更紧）
  2. 去除常见语气词开头（「噜噜」「嗯」「那」「其实」等）
  3. 去除纯重复/灌水内容（连续相同字符超过 3 个折叠为「…」）
  4. 情绪关键词匹配 → 附加情绪标签（「[急]」「[兴奋]」「[疑问]」「[指令]」）
     - 「!」或「快」「立刻」「马上去」→ ⚡指令
     - 「?」「?」或「什么」「为什么」「怎么」→ ❓疑问
     - 「哈哈哈哈」「笑死」「草」→ 😂乐
     - 「烦」「气」「怒」「操」→ 💢怒
     - 无匹配 → 不标
  5. 格式：`[发送者摘要] 提炼内容 [情绪标签]`
     - 发送者摘要：私聊用「琳玲」，群聊用「群名缩写:说话人」
     - 例如：`[蛋蛋群:蛋蛋] 神大人让葵写计划书 ⚡指令`
     - 例如：`[私聊] 心情不好，不想动 ❓疑问`
- `prompt_segment()`：注入其他 session 的近期观察（前 5 条，每条 ≤50 字，带情绪标签）

**不管的：**
- 任何记忆存储——L1/L2/L3/RAM/Disk 全在 memory
- 对话历史解析、摘要、分析
- LLM 调用

**依赖声明：** 无外部模块依赖——session 模块是纯状态管理，不依赖 memory

### MemoryModule（tremolite-memory，全链路大改）

**管的：**
- **L1：** `HashMap<String, L1Buffer>`，每个 session 独立分片
  - 只读不写——引擎读 L1 原文注入 prompt，不写回 L1
  - token 预算驱动，超了自动 pop_front 淘汰最旧的
  - 满淘汰不单独触发提炼——**攒够 N 条对话（如 10 条）才对这一批做批量提炼**
  - L1 原文进 RAM 不做全文索引，淘汰即丢弃（已经提炼过了）
- **批量提炼：** L1 攒够批 size → 纯规则提炼（去填充、情绪标签、关键实体提取、50 字以内）
  - 产出一条**带编号的信息条目** → 分配唯一 ID，携带来源 session_id + 时间窗
  - 进 L2 前做价值判断：有用 → L2；没用 → 直接丢弃
- **L2：信息池**（提炼条目 + 用户画像）
  - 晋升上限 L2——已提炼的条目不可解压回原文，不能升回 L1
  - L2 内有独立画像子池（当前活跃用户的画像在此）
  - 代谢引擎：挑使用频率低的条目 → 再次提炼为关键词 → 进 L3；完整条目 → 进 RAM
- **L3+RAM：索引 + 文件仓库**
  - L3：`HashMap<u64, String>` → `{ 编号: 关键词 }`
  - RAM：`data/ram/{编号}.txt` → 文本文件存完整提炼条目
  - 检索：L3 语义搜索命中关键词 → 返回对应编号 → 读 RAM 文件返回完整内容
  - 条目带编号指的就是这个——编号在 L3→RAM 之间一对一绑定
- **Disk：冷归档**
  - 镜像 L3+RAM 结构：`disk_index.json` → `{编号: 关键词}`；`disk_store/{编号}.txt` → 完整条目
  - 编号也一对一绑定，和 L3+RAM 共用同一套编号空间（不冲突）
  - 索引不找内存借空间——独立建目录，自己管自己
- **退化路径：** L3 不可用 → 退化到 RAM 全文搜索（contains()）；向量不可用 → 退化到关键词匹配
- **快速库 ProfileCache**（平行于 L3，独立小池）
  - 两层查询：
    ① **精确层** HashMap<QQ号/名字, 画像ID> → O(1)
       命中 → 确认是同一个人，直接返回画像
    ② **语义兜底层** 向量搜索名字/内容 → 相似度 > 阈值（如 0.9）
       命中 → 不直接套用，标记为「候选匹配」，返回标注供引擎决定是否确认
  - **两层都未命中 → 新人，空白画像，不套任何已有画像**
  - 画像也带编号，可追溯到提炼来源（「这条画像来自第 #42 条 L2 提炼条目」）
  - 更新时机：用户回应后，反思验证通过 → 写回快速库 + RAM 完整条目
  - 几百条量级，线性扫描足够，不引入独立向量数据库

**不管的：**
- session 生命周期管理
- 跨 session 通知通道
- 反思分析

**依赖声明：** 无外部模块依赖——所有输入靠 Engine 传入 session_id 和 EventContext

### Engine（tremolite-core，调适）

**改的：**
- `process_with_llm()` 中拉历史：从 `mm.recent_entries(20)` 改为 `mm.recent_entries(&self.session_id, 20)`
- prompt 构建时 collect_prompt_segments 已包含 session 模块的 ring 注入
- 协调反思流程：收到消息后并行调度 → 读 L1 原文 + ProfileCache 加载画像 → 综合 → 建议 → LLM → 验证 → 写回快速库

## 四、数据结构

### ProfileCache——快速库（平行于 L3）

```rust
/// 用户画像快速库——独立于 L3，不混用索引空间
pub struct ProfileCache {
    /// 精确命中层：QQ号/名字 → 画像ID
    exact: HashMap<String, u64>,
    /// 轻量嵌入：画像ID → 嵌入向量（线性扫描用，几百条量级）
    embeddings: HashMap<u64, Vec<f32>>,
    /// 画像本体：画像ID → 完整画像内容
    profiles: HashMap<u64, ProfileEntry>,
    max_id: u64,
}

pub struct ProfileEntry {
    pub id: u64,
    pub name: String,
    pub qq: Option<String>,
    pub content: String,            // 用户画像全文
    pub source_item_id: u64,        // 来源提炼条目编号（可追溯）
    pub updated_at: u64,
}
```

### 编号机制

```
L1(原文,无编号)
  │ ↓ 批量提炼（10条对话 → 1条条目）
  │ 分配新编号 → #42
  ↓
L2(信息条目, 携带编号#42)
  │ ↓ 代谢降级
  ├─ L3:   { 42: "debug A失败 B失败 琳玲" }
  ├─ RAM:  42.txt → 完整提炼条目
  │
  ├─ Disk索引: { 42: "debug 琳玲" }  ← 被代谢压到此层时
  └─ Disk文件: disk_store/42.txt      ← 镜像RAM结构

  ┌─ 快速库: 画像也带编号 source_item_id=#42
  │   表示「这张画像提炼自条目 #42」
  └─ 可追溯链路：#42 编号进所有下层存储，永不丢失
```

编号空间全局唯一，`MemoryManager.next_id` 分配。L3/Disk 索引条目共享编号，不会冲突（同一个条目不同层不重复分配编号）。

### MemoryManager 改造

```rust
// L1WorkingMemory 增加 token 预算管理：
pub struct L1WorkingMemory {
    buffer: VecDeque<MemoryEntry>,
    /// token 预算上限——取自模型最大上下文 × 注入比例（默认 0.7）
    token_budget: usize,
    /// 当前累计 token 数（估算）
    used_tokens: usize,
}

// 估算 token 数：中文字符算 1.3 token，英文按空格分词算 1.1 token
fn estimate_tokens(text: &str) -> usize { ... }

// push 时累计 token，超预算则 pop_front 直到 budget 够
pub fn push(&mut self, entry: MemoryEntry) {
    let tokens = estimate_tokens(&entry.content);
    while self.used_tokens + tokens > self.token_budget && !self.buffer.is_empty() {
        let old = self.buffer.pop_front().unwrap();
        self.used_tokens = self.used_tokens.saturating_sub(estimate_tokens(&old.content));
    }
    self.buffer.push_back(entry);
    self.used_tokens += tokens;
}

// MemoryManager 新增：
pub fn set_token_budget(&mut self, max_context_tokens: usize, ratio: f64) {
    let budget = (max_context_tokens as f64 * ratio.clamp(0.1, 0.9)) as usize;
    for (_, l1) in &mut self.l1_sessions {
        l1.token_budget = budget;
        l1.shrink_to_budget();  // 新 budget 比旧的小，立即压缩
    }
}
```

### MemoryManager 接口变更

| 旧接口 | 新接口 | 说明 |
|--------|--------|------|
| `remember(content, tags, imp, src)` | `remember(sid, content, tags, imp, src)` | 写对应 session L1 shard |
| `recent_entries(n)` | 删除 | 不再存在全局 L1 |
| `recent_entries_by_session(n, sid)` | → `recent_entries(sid, n)` | 提升为首要接口，n 为最大条数（预算优先，超预算时返回少于 n 条） |
| 无 | `remove_session(sid)` | 删除 L1 shard（session 过期时调用） |
| 无 | `set_token_budget(budget, ratio)` | 设置 token 预算（从 config 读取） |
| `save_l1()` | `save_l1_sessions()` | 保存全部分片为一个文件 |
| `restore_l1()` | `restore_l1_sessions()` | 恢复分片 Map |
| `metabolize()` | `metabolize()` (内部遍历) | 实现不变，遍历所有 shard |

### SessionModule（新 Module 接口）

```rust
impl Module for SessionModule {
    fn id(&self) -> &str { "session" }
    fn name(&self) -> &str { "会话管理" }
    fn required_modules(&self) -> Vec<&str> { vec!["memory"] }
    
    fn on_event(&mut self, event, ctx) -> Result<EventResponse, ModuleError> {
        match event {
            OnMessage { input, channel } => {
                self.manager.get_or_create(&ctx.session_id);
                self.ring.push(CrossSessionNote {
                    source_session: ctx.session_id.clone(),
                    content: format!("{}", input.chars().take(80).collect::<String>()),
                    created_at: now_secs(),
                });
            }
            Shutdown => {
                // persist session states (last_active)
            }
            _ => Pass
        }
    }
    
    fn prompt_segment(&self) -> Option<String> {
        let notes = self.ring.peek(5);
        if notes.is_empty() { return None; }
        // 跨 session 互通——看到其他会话的即时消息
        // 仅当有其他 session 的活跃观察时才注入
        let lines: Vec<String> = notes.iter()
            .map(|n| format!("[{}] {}", n.source_session, n.content))
            .collect();
        Some(format!("其他会话动态：\n{}", lines.join("\n")))
    }
}
```

## 六、交叉依赖

SessionModule 和 MemoryModule 不直接互相依赖。交叉通过 Engine 和 EventContext 完成：

```
OnMessage broadcast → SessionModule.touch() + 压 ring
                    → MemoryModule 写 L1（从 ctx.session_id 获取目标 shard）

process_with_llm() → Engine 统调：
  1. MemoryModule.recent_entries(session_id, 20) — 读 L1 原文
  2. SessionModule.ring.peek(5) — 读跨 session 互通
  3. ProfileCache.get(name/qq) — 加载画像
  4. 三路综合 → prompt injection
```

两个模块独立注册，不声明 `required_modules`。

## 七、Phase 分步

### Phase 1 — L1 分片 + Token 预算 + 批量提炼入口

**目标：** L1 改分片，token 预算驱动淘汰，批 size 到达触发提炼

**步骤：**
1. `MemoryManager.l1` → `l1_sessions: HashMap<String, L1Buffer>`
2. L1Buffer 加 token 预算管理：`estimate_tokens()` + push 时自动 pop_front
3. `L1.batch_size`（默认 10），每 push 一条检查累计 → 到达后调用 `distill_batch()`
4. `distill_batch()`：纯规则提炼（去填充、情绪标签、关键实体、50 字内）
5. 产出带编号信息条目 → 进 L2 前价值判断（有用/丢弃）
6. 删除旧 `recent_entries()`，`recent_entries_by_session(sid, n)` 成为主接口
7. 持久化：`save_l1_sessions()` / `restore_l1_sessions()`
8. L1 原文不写 RAM/Disk——淘汰即丢弃

**验证：** 多 session 分别积攒批 size → 各自出提炼 → L2 内检查条目来源 session_id 正确

### Phase 2 — L2 重构 + 降级链路（L2→L3→RAM）

**目标：** L2 晋升上限，代谢引擎改降级逻辑——L3 索引导入 RAM 文件

**步骤：**
1. L2 晋升上限设为 None（不可升回 L1）
2. 代谢引擎 `evaluate()`：L2 条目低使用率 → 再次提炼为关键词（极简，≤15 字）
3. L3 改为 `HashMap<u64, String>`：`{编号: 关键词}`，带语义搜索（向量退化到关键词匹配）
4. RAM 改为编号文件存储：`data/ram/{编号}.txt`
5. 检索链路：L3 语义搜索 → 命中关键词 → 返回编号 → 读 RAM 文件 → 返回完整条目
6. 降级通路：L2 → (关键词去 L3, 完整条目去 RAM)，不互相覆盖，编号共享
7. Disk 相同结构：`disk_index.json` → `{编号: 关键词}`；`disk_store/{编号}.txt` → 完整条目
8. Disk 不找内存借空间：索引自己建目录，独立管理

**验证：** L2→L3 降级后，关键词能在 L3 搜到 → 编号查 RAM 文件 → 返回正确完整条目

### Phase 3 — ProfileCache 快速库

**目标：** 用户画像独立快速库，精确命中直接返回，语义兜底只标候选不套用，陌生用户空白开始

**步骤：**
1. `ProfileCache` 实现：`exact`(HashMap)、`embeddings`(线性扫描)、`profiles`
2. 精确层 `get_by_qq(name)` → O(1) → 命中即返回画像
3. 兜底层 `search_by_embedding(text)` → 线性扫描算相似度 → 返回（命中, 候选标记）
4. 两轮皆未命中 → 返回 None（引擎侧新建空画像，不套任何已有画像）
5. 画像带 `source_item_id` 编号，链回提炼来源
6. 更新入口：用户回应后反思验证 → `ProfileCache.update()`
7. RESTful 备份：`profile_cache.json` 序列化/反序列化

**验证：** 精确命中 < 1ms；语义兜底标记候选但不套用；完全陌生用户返回 None → 引擎新建空白画像

### Phase 4 — SessionModule 注册

**目标：** SessionManager 包装为 Module，管理会话生命周期 + 跨 session ring

**步骤：**
1. `tremolite-session/Cargo.toml` 加 `tremolite-core` 依赖
2. `SessionModule` struct：持 `SessionManager` + `CrossSessionRing` + `NoteDistiller`
3. Module trait：`id="session"`，`required_modules=[]`（不再要求 memory）
4. `on_event(OnMessage)`：touch session + 提炼后压入 ring
5. `prompt_segment()`：读取 ring.peek(5)，格式化带情绪标签注入
6. `on_event(Shutdown)`：flush session states
7. `CrossSessionRing` push/peek 实现 + `NoteDistiller` 纯规则提炼
8. 主循环注册 `SessionModule`

**验证：** A session 发消息 → 提炼后进 ring → B session prompt 可见

### Phase 5 — Engine + Reflection 适配

**目标：** Engine 和反思系统适配新链路

**步骤：**
1. Engine `process_with_llm()`：读 L1（`mm.recent_entries(sid, 20)`）+ 读 L2 画像 + 读 ring
2. 反思流程实现：
   - 收到消息 → 并行：读 L1 原文 + 加载快速库画像（ProfileCache→L2）
   - 读 L2 画像 + 关键词 → 三路综合 → 生成即时建议
   - 建议 + 用户消息 → LLM → 回复
   - 用户回应后 → 反思验证建议是否被采纳 → 写回快速库
3. 建议即用即抛——不写持久库，只在这轮 prompt 生效

**验证：** 完整链路跑通：消息进 → L1→提炼→L2 + 画像加载 → 反思 → LLM → 回复 → 验证 → 更新画像

### Phase 6 — 清理与测试

1. 删除旧 `L1WorkingMemory` 中的废弃方法
2. 更新 `MemoryModule.decontaminate()` 和 `check_memory_pressure()`
3. 全工作区编译通过 + 全部测试（需重写旧测试适配新接口）
4. 手动场景：
   - 长对话场景：连续 50 条消息，L1 token 超预算 → 自动 pop_front + 批提炼触发
   - 多 session 隔离：A/B 会话各自 L1 互不污染
   - 跨 session 互通：A 发消息 → B 的 prompt 能看到提炼观察
   - 画像加载：已知用户精确命中 vs 陌生用户向量兜底
   - L3→RAM 检索：降级后查编号可追溯到完整条目

## 八、退化路径

| 故障场景 | 行为 |
|----------|------|
| session 模块未注册 | MemoryModule 的 `recent_entries()` 用 `l1_sessions.get("default")`，L1 退化为单分片 |
| session 模块挂了 | memory 不拿 ring 数据，prompt 无跨 session 注入 |
| memory 模块未注册 | session 模块收到 OnMessage 但不写入 L1（仅记录 last_active），过期清理不通知 |
| embedder 不可用 | 与 L1 分片无关，继承现有退化：RAM 搜索 fallback 到全文 contains() |
| l1_sessions.json 损坏 | 初始化空 HashMap，所有 session 从空 L1 开始 |
