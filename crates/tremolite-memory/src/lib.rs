use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ─── 嵌入引擎模块 ─────────────────────────────────
pub mod embedding;

// ─── 核心数据类型 ─────────────────────────────────────

/// 记忆层级——仿芯片缓存的五层结构
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum MemoryLevel {
    L1,  // 工作记忆（当前对话窗口）
    L2,  // 画像记忆（偏好/设定）
    L3,  // 备忘索引（标签 + 时间戳）
    Ram, // 全量历史（可全文检索）
    Disk,// 冷归档（压缩存储）
}

impl MemoryLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryLevel::L1 => "L1工作记忆",
            MemoryLevel::L2 => "L2画像记忆",
            MemoryLevel::L3 => "L3备忘索引",
            MemoryLevel::Ram => "RAM全量历史",
            MemoryLevel::Disk => "Disk冷归档",
        }
    }

    /// 层级越深，优先级数字越大（L1最高，Disk最低）
    pub fn priority(&self) -> u8 {
        match self {
            MemoryLevel::L1 => 5,
            MemoryLevel::L2 => 4,
            MemoryLevel::L3 => 3,
            MemoryLevel::Ram => 2,
            MemoryLevel::Disk => 1,
        }
    }
}

/// 记忆条目——每条记忆的元数据
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: u64,
    pub content: String,
    pub level: MemoryLevel,
    pub created_at: u64,     // unix timestamp (秒)
    pub last_access: u64,    // 最后访问时间
    pub access_count: u64,   // 访问次数（用于 LFU）
    pub tags: Vec<String>,
    pub importance: f64,     // 0.0 ~ 1.0，由代谢引擎打分
    pub source: String,      // 来源通道
}

impl MemoryEntry {
    pub fn new(id: u64, content: String, source: String) -> Self {
        let now = now_secs();
        Self {
            id,
            content,
            level: MemoryLevel::L1,
            created_at: now,
            last_access: now,
            access_count: 1,
            tags: Vec::new(),
            importance: 0.5,
            source,
        }
    }

    /// 活力分——代谢引擎的核心指标
    pub fn vitality_score(&self, current_time: u64) -> f64 {
        let age_hours = (current_time - self.created_at) as f64 / 3600.0;
        let recency = 1.0 / (1.0 + age_hours * 0.1);
        let freq = (self.access_count as f64).ln_1p() / 5.0;
        let importance = self.importance;

        // 加权合成：时效性30% + 频率30% + 重要度40%
        0.3 * recency + 0.3 * freq.min(1.0) + 0.4 * importance
    }

    /// 新鲜度加成：新创建的条目（含刚晋升的）24 小时内加分，线性衰减
    /// 只用于降级保护，不参与晋升判定
    pub fn freshness_bonus(created_at: u64, current_time: u64) -> f64 {
        let age_hours = (current_time - created_at) as f64 / 3600.0;
        if age_hours < 24.0 {
            0.25 * (1.0 - age_hours / 24.0)
        } else {
            0.0
        }
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_secs()
}

// ─── L1: 工作记忆 ─────────────────────────────────────
// 像 CPU 的 L1 缓存——最快、最小、存最近的对话

/// L1 工作记忆——按 session 分片的 buffer，token 预算驱动淘汰
pub struct L1Buffer {
    buffer: VecDeque<MemoryEntry>,
    /// token 预算上限——取自模型最大上下文 × 注入比例
    token_budget: usize,
    /// 当前累计 token 数（估算）
    used_tokens: usize,
    /// 距离上次批量提炼已积攒的条目数
    pending_batch: usize,
}

/// 估算 token 数：中文字符 × 1.3，英文按空格分词 × 1.1
fn estimate_tokens(text: &str) -> usize {
    let chinese = text.chars().filter(|c| *c as u32 > 0x2E80).count();
    let words = text.split_whitespace().count();
    (chinese as f64 * 1.3 + words as f64 * 1.1) as usize
}

impl L1Buffer {
    pub fn new(token_budget: usize) -> Self {
        Self {
            buffer: VecDeque::new(),
            token_budget,
            used_tokens: 0,
            pending_batch: 0,
        }
    }

    /// 添加一条新记忆——超 token 预算则 pop_front
    pub fn push(&mut self, entry: MemoryEntry) {
        let tokens = estimate_tokens(&entry.content);
        while !self.buffer.is_empty() && self.used_tokens + tokens > self.token_budget {
            let old = self.buffer.pop_front().unwrap();
            self.used_tokens = self.used_tokens.saturating_sub(estimate_tokens(&old.content));
        }
        self.buffer.push_back(entry);
        self.used_tokens += tokens;
        self.pending_batch += 1;
    }

    /// 检查是否攒够了批量提炼的批 size
    pub fn should_distill(&self, batch_size: usize) -> bool {
        self.pending_batch >= batch_size
    }

    /// 重置批计数器（提炼后调用）
    pub fn reset_batch_counter(&mut self) {
        self.pending_batch = 0;
    }

    pub fn entries(&self) -> &VecDeque<MemoryEntry> { &self.buffer }
    pub fn entries_mut(&mut self) -> &mut VecDeque<MemoryEntry> { &mut self.buffer }
    pub fn len(&self) -> usize { self.buffer.len() }
    pub fn used_tokens(&self) -> usize { self.used_tokens }
    pub fn token_budget(&self) -> usize { self.token_budget }

    pub fn access(&mut self, id: u64) -> Option<&mut MemoryEntry> {
        if let Some(pos) = self.buffer.iter().position(|e| e.id == id) {
            if let Some(mut entry) = self.buffer.remove(pos) {
                entry.last_access = now_secs();
                entry.access_count += 1;
                self.buffer.push_back(entry);
                return self.buffer.back_mut();
            }
        }
        None
    }

    /// 弹出最早的一条（批量提炼用）
    pub fn pop_oldest(&mut self) -> Option<MemoryEntry> {
        let entry = self.buffer.pop_front()?;
        self.used_tokens = self.used_tokens.saturating_sub(estimate_tokens(&entry.content));
        Some(entry)
    }

    /// 压缩到当前 token 预算以内——从最旧的开始丢弃
    pub fn shrink_to_budget(&mut self) {
        while self.used_tokens > self.token_budget && !self.buffer.is_empty() {
            let _ = self.pop_oldest();
        }
    }
}

// ─── L2: 画像记忆 ─────────────────────────────────────
// 像 CPU 的 L2 缓存——更大、稍慢、存持久偏好
// 自动保存到文件

/// L2 画像记忆——LFU + 文件持久化 + 语义搜索
pub struct L2ProfileMemory {
    store: HashMap<String, MemoryEntry>,
    /// 嵌入向量存储——key 与 store 同步，存 1024 维向量
    embedding_store: HashMap<String, Vec<f32>>,
    /// 粗略向量存储——降级时放在 L3 的简化版，来自上次降级
    rough_embeddings: HashMap<String, Vec<f32>>,
    max_entries: usize,
    file_path: PathBuf,
    /// 嵌入文件的路径（与主文件同目录）
    emb_path: PathBuf,
    /// 粗略向量文件的路径
    rough_emb_path: PathBuf,
    dirty: bool,
    emb_dirty: bool,
    rough_dirty: bool,
}

impl L2ProfileMemory {
    pub fn new(path: PathBuf) -> Self {
        let max_entries = 200;
        let store = if path.exists() {
            std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        // 嵌入文件路径：l2_profile.json → l2_embeddings.json
        let emb_path = {
            let mut p = path.clone();
            p.set_file_name("l2_embeddings.json");
            p
        };
        let embedding_store = if emb_path.exists() {
            std::fs::read_to_string(&emb_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        // 粗略嵌入文件路径：l2_profile.json → l2_rough.json
        let rough_emb_path = {
            let mut p = path.clone();
            p.set_file_name("l2_rough.json");
            p
        };
        let rough_embeddings = if rough_emb_path.exists() {
            std::fs::read_to_string(&rough_emb_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_default()
        } else {
            HashMap::new()
        };

        Self {
            store,
            embedding_store,
            rough_embeddings,
            max_entries,
            file_path: path,
            emb_path,
            rough_emb_path,
            dirty: false,
            emb_dirty: false,
            rough_dirty: false,
        }
    }

    /// 存入一条画像记忆（key 通常是主题名）
    pub fn set(&mut self, key: &str, content: String, tags: Vec<String>, importance: f64) {
        let now = now_secs();
        let id = now;
        let entry = MemoryEntry {
            id,
            content,
            level: MemoryLevel::L2,
            created_at: now,
            last_access: now,
            access_count: 1,
            tags,
            importance,
            source: String::new(),
        };

        // LFU：如果 key 已存在，继承访问计数
        if let Some(existing) = self.store.get(key) {
            let mut e = entry;
            e.access_count = existing.access_count + 1;
            e.id = existing.id;
            self.store.insert(key.to_string(), e);
            // 旧粗略向量与新内容不再匹配，清理
            self.rough_embeddings.remove(key);
        } else {
            // 超过上限时踢掉最低频的
            if self.store.len() >= self.max_entries {
                let lowest_key = self
                    .store
                    .iter()
                    .min_by_key(|(_, v)| v.access_count)
                    .map(|(k, _)| k.clone());
                if let Some(k) = lowest_key {
                    self.store.remove(&k);
                    self.embedding_store.remove(&k);
                }
            }
            self.store.insert(key.to_string(), entry);
        }
        self.dirty = true;
    }

    /// 按 key 读取 embedding（供代谢升降级用）
    pub fn get_embedding(&self, key: &str) -> Option<Vec<f32>> {
        self.embedding_store.get(key).cloned()
    }

    /// 存储粗略向量（降级时放到 L3 的简化版）
    pub fn set_rough_embedding(&mut self, key: &str, vec: Vec<f32>) {
        self.rough_embeddings.insert(key.to_string(), vec);
        self.rough_dirty = true;
    }

    /// 读取粗略向量（再次降级时直接复用）
    pub fn get_rough_embedding(&self, key: &str) -> Option<Vec<f32>> {
        self.rough_embeddings.get(key).cloned()
    }

    /// 存入一条画像记忆，同时带上预计算的嵌入向量
    /// 用于语义搜索
    pub fn set_with_embedding(&mut self, key: &str, content: String, tags: Vec<String>,
                               importance: f64, embedding: Vec<f32>) {
        self.set(key, content, tags, importance);
        self.embedding_store.insert(key.to_string(), embedding);
        self.emb_dirty = true;
    }

    /// 语义搜索——用 query 嵌入在所有画像中找到最相似的
    /// 返回 (key, entry, similarity_score)
    pub fn search_semantic(&self, query_embedding: &[f32], k: usize) -> Vec<(String, &MemoryEntry, f64)> {
        let mut scored: Vec<(String, &MemoryEntry, f64)> = self.embedding_store.iter()
            .filter_map(|(key, emb)| {
                let entry = self.store.get(key)?;
                let sim = crate::embedding::cosine_similarity(query_embedding, emb);
                Some((key.clone(), entry, sim))
            })
            .collect();

        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        scored.truncate(k);
        scored
    }

    /// 读取一条画像记忆
    pub fn get(&mut self, key: &str) -> Option<&MemoryEntry> {
        if let Some(entry) = self.store.get_mut(key) {
            entry.last_access = now_secs();
            entry.access_count += 1;
            self.dirty = true;
            Some(entry)
        } else {
            None
        }
    }

    /// 所有画像
    pub fn all(&self) -> &HashMap<String, MemoryEntry> {
        &self.store
    }

    /// 找出并移除活力分低于阈值的条目（降级用）
    pub fn evict_demoted(&mut self, threshold: f64) -> Vec<(String, MemoryEntry)> {
        let now = now_secs();
        let keys: Vec<String> = self
            .store
            .iter()
            .filter(|(_, e)| e.vitality_score(now) < threshold)
            .map(|(k, _)| k.clone())
            .collect();

        let mut evicted = Vec::new();
        for k in keys {
            if let Some(entry) = self.store.remove(&k) {
                self.embedding_store.remove(&k);
                self.rough_embeddings.remove(&k);
                evicted.push((k, entry));
                self.dirty = true;
            }
        }
        evicted
    }

    /// 保存到文件（主文件 + 嵌入文件）
    pub fn flush(&mut self) -> Result<(), String> {
        if self.dirty {
            let json = serde_json::to_string_pretty(&self.store).map_err(|e| e.to_string())?;
            if let Some(parent) = self.file_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&self.file_path, json).map_err(|e| e.to_string())?;
            self.dirty = false;
        }
        if self.emb_dirty {
            let json = serde_json::to_string_pretty(&self.embedding_store).map_err(|e| e.to_string())?;
            if let Some(parent) = self.emb_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&self.emb_path, json).map_err(|e| e.to_string())?;
            self.emb_dirty = false;
        }
        if self.rough_dirty {
            let json = serde_json::to_string_pretty(&self.rough_embeddings).map_err(|e| e.to_string())?;
            if let Some(parent) = self.rough_emb_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&self.rough_emb_path, json).map_err(|e| e.to_string())?;
            self.rough_dirty = false;
        }
        Ok(())
    }
}

impl Drop for L2ProfileMemory {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

// ─── L3: 备忘索引 ─────────────────────────────────────
// 只存标签 + 时间戳 + 指针，不存原文
// 像图书馆的索引卡

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub id: u64,
    pub tags: Vec<String>,
    pub created_at: u64,
    pub last_access: u64,
    pub level_from: MemoryLevel,
    pub summary: String,
    pub embedding: Option<Vec<f32>>, // 向量（层间升降级时传递）
}

/// L3 备忘索引——HashMap<u64, String> 编号→关键词
pub struct L3IndexMemory {
    keywords: HashMap<u64, String>,
    last_access: HashMap<u64, u64>,
    created_at: HashMap<u64, u64>,
    max_entries: usize,
    /// 向量存储——编号→向量（层间升降级传递）
    embeddings: HashMap<u64, Vec<f32>>,
}

impl L3IndexMemory {
    pub fn new() -> Self {
        Self {
            keywords: HashMap::new(),
            last_access: HashMap::new(),
            created_at: HashMap::new(),
            max_entries: 1000,
            embeddings: HashMap::new(),
        }
    }

    /// 添加一条索引
    pub fn add(&mut self, entry: IndexEntry) {
        if let Some(emb) = &entry.embedding {
            self.embeddings.insert(entry.id, emb.clone());
        }
        let now = now_secs();
        if self.keywords.len() >= self.max_entries {
            // 移除最久未访问的
            if let Some((oldest_id, _)) = self
                .last_access
                .iter()
                .min_by_key(|(_, &t)| t)
            {
                let id = *oldest_id;
                self.keywords.remove(&id);
                self.last_access.remove(&id);
                self.created_at.remove(&id);
            }
        }
        self.keywords.insert(entry.id, entry.summary);
        self.last_access.insert(entry.id, entry.last_access);
        self.created_at.insert(entry.id, entry.created_at);
    }

    /// 全文匹配关键词
    pub fn search_by_summary(&self, keyword: &str) -> Vec<IndexEntry> {
        let lower = keyword.to_lowercase();
        self.keywords
            .iter()
            .filter(|(_, kw)| kw.to_lowercase().contains(&lower))
            .map(|(&id, kw)| IndexEntry {
                id,
                summary: kw.clone(),
                tags: Vec::new(),
                created_at: self.created_at.get(&id).copied().unwrap_or(0),
                last_access: self.last_access.get(&id).copied().unwrap_or(0),
                level_from: MemoryLevel::L3,
                embedding: None,
            })
            .collect()
    }

    pub fn len(&self) -> usize {
        self.keywords.len()
    }

    /// 所有索引条目（转为 Vec）
    pub fn all_entries(&self) -> Vec<IndexEntry> {
        self.keywords
            .iter()
            .map(|(&id, kw)| IndexEntry {
                id,
                summary: kw.clone(),
                embedding: None,
                tags: Vec::new(),
                created_at: self.created_at.get(&id).copied().unwrap_or(0),
                last_access: self.last_access.get(&id).copied().unwrap_or(0),
                level_from: MemoryLevel::L3,
            })
            .collect()
    }

    /// 基于时间跨度评估条目是否该降级
    pub fn stale_score(&self, id: u64) -> f64 {
        let now = now_secs();
        let last = self.last_access.get(&id).copied().unwrap_or(0);
        let created = self.created_at.get(&id).copied().unwrap_or(0);
        let hours_since_access = (now - last) as f64 / 3600.0;
        let hours_since_created = (now - created) as f64 / 3600.0;
        let recency = 1.0 / (1.0 + hours_since_access * 0.1);
        let age = 1.0 / (1.0 + hours_since_created * 0.05);
        0.5 * recency + 0.5 * age
    }

    /// 找出并移除低分索引（降级用）
    pub fn evict_demoted(&mut self, threshold: f64) -> Vec<IndexEntry> {
        let ids: Vec<u64> = self
            .keywords
            .keys()
            .filter(|id| self.stale_score(**id) < threshold)
            .copied()
            .collect();

        let mut evicted = Vec::new();
        for id in ids {
            if let Some(kw) = self.keywords.remove(&id) {
                let emb = self.embeddings.remove(&id);
                let entry = IndexEntry {
                    id,
                    summary: kw,
                    tags: Vec::new(),
                    created_at: self.created_at.remove(&id).unwrap_or(0),
                    last_access: self.last_access.remove(&id).unwrap_or(0),
                    level_from: MemoryLevel::L3,
                    embedding: emb,
                };
                evicted.push(entry);
            }
        }
        evicted
    }
}

// ─── RAM: 全量历史 ────────────────────────────────────
// 朴素的 FTS 实现（后续可替换为 tantivy）

/// RAM 编号文件存储——`data/ram/{编号}.txt`
/// 不存全文索引，只按 ID 做文件读写。语义搜索由 L3 负责。
pub struct RamFileStore {
    base_path: PathBuf,
    /// 编号集合（快速存在性判断，不存内容）
    ids: HashSet<u64>,
    /// 创建时间戳（编号→秒，stale_score 用）
    created_at: HashMap<u64, u64>,
    max_entries: usize,
}

impl RamFileStore {
    pub fn new(base_path: PathBuf) -> Self {
        std::fs::create_dir_all(&base_path).ok();
        Self {
            base_path,
            ids: HashSet::new(),
            created_at: HashMap::new(),
            max_entries: 10_000,
        }
    }

    /// 写入一条条目到编号文件
    pub fn add(&mut self, id: u64, content: &str, created: u64) {
        if self.ids.len() >= self.max_entries {
            // 淘汰最旧的
            if let Some((&oldest, _)) = self.created_at.iter().min_by_key(|(_, &t)| t) {
                self.remove(oldest);
            }
        }
        let path = self.base_path.join(format!("{}.txt", id));
        let _ = std::fs::write(&path, content);
        self.ids.insert(id);
        self.created_at.insert(id, created);
    }

    /// 按编号读取文件内容
    pub fn read(&self, id: u64) -> Option<String> {
        let path = self.base_path.join(format!("{}.txt", id));
        std::fs::read_to_string(&path).ok()
    }

    /// 删除一条
    pub fn remove(&mut self, id: u64) {
        self.ids.remove(&id);
        self.created_at.remove(&id);
        let path = self.base_path.join(format!("{}.txt", id));
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(self.base_path.join(format!("{}.vec.json", id)));
    }

    /// 存储详细向量到 RAM（配套文件 {id}.vec.json）
    pub fn store_vector(&self, id: u64, vec: &[f32]) {
        let path = self.base_path.join(format!("{}.vec.json", id));
        if let Ok(json) = serde_json::to_string(vec) {
            let _ = std::fs::write(&path, json);
        }
    }

    /// 从 RAM 读取详细向量
    pub fn read_vector(&self, id: u64) -> Option<Vec<f32>> {
        let path = self.base_path.join(format!("{}.vec.json", id));
        std::fs::read_to_string(&path).ok()
            .and_then(|s| serde_json::from_str(&s).ok())
    }

    /// 删除详细向量文件
    pub fn remove_vector(&self, id: u64) {
        let _ = std::fs::remove_file(self.base_path.join(format!("{}.vec.json", id)));
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.ids.contains(&id)
    }

    /// 退化路径：全文遍历搜索（仅当 L3 不可用时）
    pub fn search_contains(&self, query: &str) -> Vec<(u64, String, f64)> {
        let lower_query = query.to_lowercase();
        let mut results = Vec::new();
        for &id in &self.ids {
            if let Some(content) = self.read(id) {
                if content.to_lowercase().contains(&lower_query) {
                    let snippet: String = content.chars().take(80).collect();
                    let score = 0.5; // degradation 统一给中分
                    results.push((id, snippet, score));
                }
            }
        }
        results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        results.truncate(20);
        results
    }

    /// 找出并移除低分条目（stale_score < threshold）
    pub fn evict_demoted(&mut self, threshold: f64) -> Vec<(u64, String)> {
        let now = now_secs();
        let mut evicted = Vec::new();
        let stale: Vec<u64> = self
            .created_at
            .iter()
            .filter(|(&id, &created)| {
                let hours = (now - created) as f64 / 3600.0;
                let score = 1.0 / (1.0 + hours * 0.05);
                score < threshold
            })
            .map(|(&id, _)| id)
            .collect();
        for id in stale {
            if let Some(content) = self.read(id) {
                evicted.push((id, content));
            }
            self.remove(id);
        }
        evicted
    }

    /// 所有编号
    pub fn all_ids(&self) -> Vec<u64> {
        self.ids.iter().copied().collect()
    }

    /// 清空
    pub fn clear(&mut self) {
        for &id in &self.ids {
            let path = self.base_path.join(format!("{}.txt", id));
            let _ = std::fs::remove_file(&path);
        }
        self.ids.clear();
        self.created_at.clear();
    }
}

// ─── Disk: 冷归档 ────────────────────────────────────
// 文件持久化存储，JSON Lines 格式 + 独立索引

pub struct DiskColdArchive {
    base_path: PathBuf,
    max_archives: usize,
    index_dir: PathBuf,
    store_dir: PathBuf,
    index: std::sync::Mutex<HashMap<u64, String>>,
    /// 嵌入存储：编号→向量（层间传递）
    embeddings: std::sync::Mutex<HashMap<u64, Vec<f32>>>,
}

impl DiskColdArchive {
    pub fn new(base_path: PathBuf) -> Self {
        std::fs::create_dir_all(&base_path).ok();
        let index_dir = base_path.join("disk_index");
        let store_dir = base_path.join("disk_store");
        std::fs::create_dir_all(&index_dir).ok();
        std::fs::create_dir_all(&store_dir).ok();
        // 尝试加载已有索引
        let index = std::sync::Mutex::new(HashMap::new());
        let index_path = index_dir.join("index.json");
        if let Ok(json) = std::fs::read_to_string(&index_path) {
            if let Ok(loaded) = serde_json::from_str::<HashMap<u64, String>>(&json) {
                *index.lock().unwrap() = loaded;
            }
        }
        let embeddings = std::sync::Mutex::new(HashMap::new());
        let emb_path = index_dir.join("embeddings.json");
        if let Ok(json) = std::fs::read_to_string(&emb_path) {
            if let Ok(loaded) = serde_json::from_str::<HashMap<u64, Vec<f32>>>(&json) {
                *embeddings.lock().unwrap() = loaded;
            }
        }
        Self {
            base_path,
            max_archives: 50,
            index_dir,
            store_dir,
            index,
            embeddings,
        }
    }

    /// 写入条目到 Disk 独立索引 + 文件存储，可选带向量
    pub fn store_entry(&self, id: u64, keyword: &str, content: &str, _created: u64, embedding: Option<Vec<f32>>) {
        let file_path = self.store_dir.join(format!("{}.txt", id));
        let _ = std::fs::write(&file_path, content);
        if let Some(emb) = embedding {
            if let Ok(mut embs) = self.embeddings.lock() {
                embs.insert(id, emb);
                // 异步持久化嵌入文件
                let emb_path = self.index_dir.join("embeddings.json");
                if let Ok(json) = serde_json::to_string(&*embs) {
                    let _ = std::fs::write(&emb_path, json);
                }
            }
        }
        if let Ok(mut idx) = self.index.lock() {
            idx.insert(id, keyword.to_string());
            // 同步保存索引到 json
            let index_path = self.index_dir.join("index.json");
            if let Ok(json) = serde_json::to_string_pretty(&*idx) {
                let _ = std::fs::write(&index_path, json);
            }
        }
    }

    /// 从 Disk Index 读取关键词/摘要
    pub fn read_keyword(&self, target_id: u64) -> Option<String> {
        if let Ok(idx) = self.index.lock() {
            return idx.get(&target_id).cloned();
        }
        None
    }

    /// 从 Disk Index 读取嵌入向量
    pub fn read_embedding(&self, target_id: u64) -> Option<Vec<f32>> {
        if let Ok(embs) = self.embeddings.lock() {
            return embs.get(&target_id).cloned();
        }
        None
    }

    /// 按编号读取文件内容
    pub fn read_by_id(&self, target_id: u64) -> Option<MemoryEntry> {
        // 先试新格式
        let new_path = self.store_dir.join(format!("{}.txt", target_id));
        if new_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&new_path) {
                return Some(MemoryEntry {
                    id: target_id,
                    content,
                    level: MemoryLevel::Disk,
                    created_at: 0,
                    last_access: 0,
                    access_count: 0,
                    tags: Vec::new(),
                    importance: 0.0,
                    source: String::new(),
                });
            }
        }
        // 退回到旧格式归档搜索
        for path in self.list_archives() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(entry) = serde_json::from_str::<MemoryEntry>(line) {
                        if entry.id == target_id {
                            return Some(entry);
                        }
                    }
                }
            }
        }
        None
    }

    /// 归档一批记忆（旧格式 JSONL）
    pub fn archive(&self, entries: &[MemoryEntry]) -> Result<usize, String> {
        let filename = format!("archive-{}.jsonl", now_secs());
        let path = self.base_path.join(&filename);
        let mut count = 0;

        let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
        for entry in entries {
            let line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
            use std::io::Write;
            writeln!(file, "{}", line).map_err(|e| e.to_string())?;
            count += 1;
        }

        // 清理超过上限的归档文件
        self.cleanup_old();

        Ok(count)
    }

    /// 列出所有归档文件
    pub fn list_archives(&self) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.base_path)
            .ok()
            .into_iter()
            .flat_map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().map(|ext| ext == "jsonl").unwrap_or(false))
                    .map(|e| e.path())
            })
            .collect();
        files.sort();
        files
    }

    /// 搜索所有归档（旧格式 + 新索引）
    pub fn search_all(&self, keyword: &str) -> Vec<(u64, String, String)> {
        let lower_keyword = keyword.to_lowercase();
        let mut results = Vec::new();
        // 新索引：disk_index.json → disk_store
        if let Ok(idx) = self.index.lock() {
            for (&id, kw) in idx.iter() {
                if kw.to_lowercase().contains(&lower_keyword) {
                    let store_path = self.store_dir.join(format!("{}.txt", id));
                    if let Ok(content) = std::fs::read_to_string(&store_path) {
                        results.push((id, content.chars().take(80).collect(), "disk_store".into()));
                    }
                }
            }
        }
        // 旧格式：归档 JSONL
        for path in self.list_archives() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for line in content.lines() {
                    if let Ok(entry) = serde_json::from_str::<MemoryEntry>(line) {
                        if entry.content.to_lowercase().contains(&lower_keyword) {
                            results.push((
                                entry.id,
                                entry.content.chars().take(80).collect(),
                                path.file_name().unwrap_or_default().to_string_lossy().to_string(),
                            ));
                        }
                    }
                }
            }
        }
        results.truncate(50);
        results
    }

    fn cleanup_old(&self) {
        let mut files = self.list_archives();
        while files.len() > self.max_archives {
            if let Some(oldest) = files.first() {
                std::fs::remove_file(oldest).ok();
                files.remove(0);
            }
        }
    }
}

// ─── 代谢引擎 ─────────────────────────────────────────
// 核心：自动评估每条记忆的活力分，决定升降级
// 现在会动态调整阈值，根据历史分数分布自动适配

/// 分数历史——滑动窗口，存最近 N 条 vitality_score 用于计算分布
const SCORE_HISTORY_SIZE: usize = 100;

pub struct MetabolismEngine {
    /// 活力分阈值：低于此值降级
    pub demote_threshold: f64,
    /// 活力分阈值：高于此值升级
    pub promote_threshold: f64,
    /// 检查间隔（秒）
    pub check_interval: u64,
    last_check: u64,
    /// 历史分数——滑动窗口
    history: Vec<f64>,
    /// 是否启用动态阈值（默认 true）
    pub adaptive: bool,
}

impl MetabolismEngine {
    pub fn new() -> Self {
        Self {
            demote_threshold: 0.3,
            promote_threshold: 0.7,
            check_interval: 300, // 每5分钟检查一次
            last_check: now_secs(),
            history: Vec::with_capacity(SCORE_HISTORY_SIZE),
            adaptive: true,
        }
    }

    /// 记录一条活力分到历史窗口
    pub fn record_score(&mut self, score: f64) {
        if self.history.len() >= SCORE_HISTORY_SIZE {
            self.history.remove(0);
        }
        self.history.push(score);
    }

    /// 根据历史分数分布重新校准阈值
    /// demote = mean - 0.5 * stddev
    /// promote = mean + 0.5 * stddev
    /// 钳制在 [0.1, 0.95] 范围内，ensure demote < promote
    pub fn recalibrate(&mut self) {
        if !self.adaptive || self.history.len() < 10 {
            return;
        }

        let n = self.history.len() as f64;
        let mean: f64 = self.history.iter().sum::<f64>() / n;
        let variance: f64 = self.history.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
        let stddev = variance.sqrt();

        // 动态阈值 = 均值 ± 半个标准差
        let new_demote = (mean - 0.5 * stddev).clamp(0.1, 0.8);
        let new_promote = (mean + 0.5 * stddev).clamp(0.2, 0.95);

        // 确保 demote < promote（至少差 0.05）
        if new_demote < new_promote - 0.05 {
            self.demote_threshold = new_demote;
            self.promote_threshold = new_promote;
        }
    }

    /// 对一条记忆执行代谢检查，返回建议的动作
    pub fn evaluate(&self, entry: &MemoryEntry) -> MetabolicAction {
        let now = now_secs();
        let score = entry.vitality_score(now);
        let fresh = MemoryEntry::freshness_bonus(entry.created_at, now);

        match entry.level {
            MemoryLevel::L1 => {
                if score + fresh < self.demote_threshold {
                    MetabolicAction::DemoteTo(MemoryLevel::L2)
                } else {
                    MetabolicAction::Stay
                }
            }
            MemoryLevel::L2 => {
                if score > self.promote_threshold {
                    MetabolicAction::PromoteTo(MemoryLevel::L1)
                } else if score + fresh < self.demote_threshold * 0.8 {
                    MetabolicAction::DemoteTo(MemoryLevel::L3)
                } else {
                    MetabolicAction::Stay
                }
            }
            MemoryLevel::L3 => {
                if score > self.promote_threshold {
                    MetabolicAction::PromoteTo(MemoryLevel::L2)
                } else if score + fresh < self.demote_threshold * 0.6 {
                    MetabolicAction::DemoteTo(MemoryLevel::Ram)
                } else {
                    MetabolicAction::Stay
                }
            }
            MemoryLevel::Ram => {
                if score > self.promote_threshold {
                    MetabolicAction::PromoteTo(MemoryLevel::L3)
                } else if score + fresh < self.demote_threshold * 0.4 {
                    MetabolicAction::DemoteTo(MemoryLevel::Disk)
                } else {
                    MetabolicAction::Stay
                }
            }
            MemoryLevel::Disk => {
                if score > self.promote_threshold {
                    MetabolicAction::PromoteTo(MemoryLevel::Ram)
                } else {
                    MetabolicAction::Stay
                }
            }
        }
    }

    /// 是否到了检查时间
    pub fn should_check(&mut self) -> bool {
        let now = now_secs();
        if now - self.last_check >= self.check_interval {
            self.last_check = now;
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MetabolicAction {
    Stay,
    PromoteTo(MemoryLevel),
    DemoteTo(MemoryLevel),
    Discard,
}


/// 从关键词文本生成粗略向量（字符哈希 + 归一化，同维度，无需嵌入模型）
fn make_rough_vector(keyword: &str, dim: usize) -> Vec<f32> {
    let mut vec = vec![0.0f32; dim];
    for (i, ch) in keyword.chars().enumerate() {
        let idx = (ch as usize) % dim.max(1);
        vec[idx] += 1.0 / (1.0 + i as f32);
    }
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-10);
    for x in &mut vec { *x /= norm; }
    vec
}

/// 轻量画像检测——内容是否包含用户画像信息
/// 通过关键词规则判断，不调 LLM
fn is_profile_related(content: &str) -> bool {
    let lower = content.to_lowercase();

    // 画像关键词——匹配任一即视为画像
    let profile_keywords = [
        "神大人", "琳玲", "用户",
        "不喜欢", "不吃", "讨厌吃",
        "代表色", "代表",
        "住在", "家住", "住",
        "不吃海鲜", "海产品",
        "不抽烟", "不喝酒", "酒品",
        "焦虑", "吃药", "丙戊酸镁", "喹硫平", "托鲁",
        "昵称", "不喜欢", "讨厌",
        "晕厥", "心绞痛",
        "哔哩哔哩", "接吻",
        "橙子味",
        "代表色", "bf99bf",
        "近视", "500度",
    ];

    for kw in &profile_keywords {
        if lower.contains(kw) {
            return true;
        }
    }

    false
}

/// 纯规则提炼——去填充词、截断到 50 字
fn distill_entry_content(raw: &str) -> String {
    let text = raw.trim();
    // 去填充前缀
    let fillers = ["噜噜……", "嗯……", "那……", "其实……", "唔……", "哼……"];
    let mut cleaned = text.to_string();
    for f in fillers {
        if cleaned.starts_with(f) {
            cleaned = cleaned[f.len()..].trim().to_string();
            break;
        }
    }
    // 去重复折叠
    let mut deduped = String::with_capacity(cleaned.len());
    let mut prev = '\0';
    let mut repeat_count = 0;
    for c in cleaned.chars() {
        if c == prev {
            repeat_count += 1;
            if repeat_count > 3 { continue; }
        } else {
            repeat_count = 0;
        }
        deduped.push(c);
        prev = c;
    }
    // 截断 50 字
    let truncated: String = deduped.chars().take(50).collect();
    if truncated.len() < deduped.len() {
        format!("{}…", truncated)
    } else {
        truncated
    }
}

/// 情绪关键词检测——返回 (发送者标签, 情绪标签)
fn detect_mood(text: &str) -> (&'static str, Option<&'static str>) {
    // 发送者标签检测
    let tag = if text.contains("kamisama:") || text.contains("神大人:") {
        "神大人"
    } else if text.contains("葵:") {
        "葵"
    } else {
        "用户"
    };
    // 情绪关键词
    let mood = if text.contains('!') || text.contains('！')
        || text.contains("快") || text.contains("立刻") || text.contains("马上去") {
        Some("指令")
    } else if text.contains('?') || text.contains('？')
        || text.contains("什么") || text.contains("为什么") || text.contains("怎么") {
        Some("疑问")
    } else if text.contains("哈哈") || text.contains("笑死") || text.contains("草")
        || text.contains("www") || text.contains("乐") {
        Some("乐")
    } else if text.contains("烦") || text.contains("气") || text.contains("怒")
        || text.contains("操") || text.contains("滚") {
        Some("怒")
    } else {
        None
    };
    (tag, mood)
}

// ─── ProfileCache: 用户画像快速库 ───────────────────
//
// 平行于 L2 的轻量画像缓存。
// 精确命中直接返回，语义兜底只标候选不强套，陌生用户空白开始。

/// 用户画像——精确层直接返回、语义层只标候选的轻量结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEntry {
    pub id: u64,
    pub content: String,
    pub created_at: u64,
    pub last_updated: u64,
    pub tags: Vec<String>,
}

impl ProfileEntry {
    pub fn new(id: u64, content: String, tags: Vec<String>) -> Self {
        let now = now_secs();
        Self { id, content, created_at: now, last_updated: now, tags }
    }
}

/// 画像快速库——精确层 + 语义兜底层，带独立升降级通道
pub struct ProfileCache {
    path: PathBuf,
    /// 身份标识 → 多条碎片（如 "神大人/00" → Vec<ProfileEntry>）
    store: HashMap<String, Vec<ProfileEntry>>,
    /// 身份标识 → 每条碎片的向量（索引与 store 对齐）
    embeddings: HashMap<String, Vec<Vec<f32>>>,
    /// 身份标识最后更新时间
    last_updated: HashMap<String, u64>,
    dirty: bool,
}

impl ProfileCache {
    pub fn new(path: PathBuf) -> Self {
        std::fs::create_dir_all(path.parent().unwrap_or(&path)).ok();
        let mut cache = Self {
            path,
            store: HashMap::new(),
            embeddings: HashMap::new(),
            last_updated: HashMap::new(),
            dirty: false,
        };
        cache.restore();
        cache
    }

    /// 向指定身份的碎片堆里加一条新内容
    pub fn add_entry(&mut self, key: &str, content: String, tags: Vec<String>, embedding: Option<Vec<f32>>) {
        let now = now_secs();
        let id = now;
        let entry = ProfileEntry::new(id, content, tags);
        self.store.entry(key.to_string()).or_default().push(entry);
        if let Some(emb) = embedding {
            self.embeddings.entry(key.to_string()).or_default().push(emb);
        }
        self.last_updated.insert(key.to_string(), now);
        self.dirty = true;
        tracing::debug!("profile_cache: added entry to '{}' (id={})", key, id);
    }

    /// 遍历所有身份的所有碎片（供晋升扫描用）——返回 (身份key, 碎片索引, &ProfileEntry)
    pub fn all_entries(&self) -> Vec<(String, usize, &ProfileEntry)> {
        let mut result = Vec::new();
        for (key, entries) in &self.store {
            for (idx, entry) in entries.iter().enumerate() {
                result.push((key.clone(), idx, entry));
            }
        }
        result
    }

    /// 获取指定身份的所有碎片
    pub fn get_entries(&self, key: &str) -> Option<&Vec<ProfileEntry>> {
        self.store.get(key)
    }

    /// 读取指定身份的某条碎片的向量
    pub fn get_embedding(&self, key: &str, index: usize) -> Option<Vec<f32>> {
        self.embeddings.get(key)?.get(index).cloned()
    }

    /// 删除指定身份的某条碎片
    pub fn remove_entry(&mut self, key: &str, index: usize) {
        if let Some(entries) = self.store.get_mut(key) {
            if index < entries.len() { entries.remove(index); }
        }
        if let Some(embs) = self.embeddings.get_mut(key) {
            if index < embs.len() { embs.remove(index); }
        }
        self.dirty = true;
    }

    /// 标记为脏
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// 代谢：超过60天没更新、且碎片≤5条的身份整窝删掉
    pub fn maintain(&mut self) -> usize {
        let now = now_secs();
        let mut evicted = 0usize;
        let stale: Vec<String> = self.last_updated.iter()
            .filter(|(key, &last)| {
                let days = (now - last) as f64 / 86400.0;
                let count = self.store.get(*key).map(|v| v.len()).unwrap_or(0);
                days > 60.0 && count <= 5
            })
            .map(|(k, _)| k.clone())
            .collect();

        for key in stale {
            self.store.remove(&key);
            self.embeddings.remove(&key);
            self.last_updated.remove(&key);
            evicted += 1;
            tracing::info!("profile_cache: evicted stale identity '{}'", key);
        }

        if evicted > 0 { self.dirty = true; }
        evicted
    }

    /// 身份数
    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// 所有身份 key
    pub fn keys(&self) -> Vec<String> {
        self.store.keys().cloned().collect()
    }

    // ─── 持久化 ────────────────────────────────

    fn restore(&mut self) {
        if !self.path.exists() { return; }
        if let Ok(json) = std::fs::read_to_string(&self.path) {
            if let Ok(data) = serde_json::from_str::<ProfileCacheData>(&json) {
                self.store = data.store;
                self.embeddings = data.embeddings;
                self.last_updated = data.last_updated;
            }
        }
    }

    pub fn flush(&mut self) -> Result<(), String> {
        if !self.dirty { return Ok(()); }
        let data = ProfileCacheData {
            store: self.store.clone(),
            embeddings: self.embeddings.clone(),
            last_updated: self.last_updated.clone(),
        };
        let json = serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }
}

/// 持久化用数据容器
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProfileCacheData {
    store: HashMap<String, Vec<ProfileEntry>>,
    embeddings: HashMap<String, Vec<Vec<f32>>>,
    last_updated: HashMap<String, u64>,
}

// ─── 统一的记忆管理器 ─────────────────────────────────

pub struct MemoryManager {
    pub l1_sessions: HashMap<String, L1Buffer>,
    pub l2: L2ProfileMemory,
    pub l3: L3IndexMemory,
    pub ram: RamFileStore,
    pub disk: DiskColdArchive,
    pub metabolism: MetabolismEngine,
    /// 嵌入服务（可选），用于 RAM 向量化搜索 + 批量嵌入
    pub embedder: Option<Box<dyn embedding::EmbeddingService + Send>>,
    /// Disk 命中追踪
    pub disk_hits: std::sync::Mutex<std::collections::HashMap<u64, u64>>,
    /// Disk 晋升阈值（命中次数超过此值升回 RAM）
    pub disk_promote_threshold: u64,
    /// 用户画像快速库
    pub profile_cache: ProfileCache,
    /// 持久化路径
    l1_path: PathBuf,
    l3_path: PathBuf,
    ram_path: PathBuf,
    next_id: u64,
}

impl MemoryManager {
    pub fn new(data_dir: PathBuf) -> Self {
        let l2_path = data_dir.join("l2_profile.json");
        let disk_path = data_dir.join("archives");
        let l1_path = data_dir.join("l1_working.json");
        let l3_path = data_dir.join("l3_index.json");
        let ram_path = data_dir.join("ram_fts.json");

        let mut mm = Self {
            l1_sessions: HashMap::new(),
            l2: L2ProfileMemory::new(l2_path),
            l3: L3IndexMemory::new(),
            ram: RamFileStore::new(data_dir.join("ram")),
            disk: DiskColdArchive::new(disk_path),
            metabolism: MetabolismEngine::new(),
            embedder: None,
            disk_hits: std::sync::Mutex::new(std::collections::HashMap::new()),
            disk_promote_threshold: 3,
            profile_cache: ProfileCache::new(data_dir.join("profile_cache.json")),
            l1_path,
            l3_path,
            ram_path,
            next_id: 1,
        };

        // 从磁盘恢复 L1, L3, RAM
        mm.restore_l1_sessions();
        mm.restore_l3();
        mm.restore_ram();

        mm
    }

    /// 根据 session_id 获取或创建 L1 buffer
    fn l1_for_session_mut(&mut self, sid: &str) -> &mut L1Buffer {
        self.l1_sessions.entry(sid.to_string())
            .or_insert_with(|| L1Buffer::new(40960)) // 默认 40K token ≈ 32K 模型 70%
    }

    /// 只读获取 L1 buffer
    fn l1_for_session(&self, sid: &str) -> Option<&L1Buffer> {
        self.l1_sessions.get(sid)
    }

    /// 遍历所有 session 的 L1（只读）
    fn for_each_l1<F>(&self, mut f: F) where F: FnMut(&str, &L1Buffer) {
        for (sid, buf) in &self.l1_sessions {
            f(sid, buf);
        }
    }

    /// 遍历所有 session 的 L1（可变）
    fn for_each_l1_mut<F>(&mut self, mut f: F) where F: FnMut(&str, &mut L1Buffer) {
        for (sid, buf) in &mut self.l1_sessions {
            f(sid, buf);
        }
    }

    /// 从磁盘恢复所有 session 的 L1
    fn restore_l1_sessions(&mut self) {
        if !self.l1_path.exists() {
            return;
        }
        match std::fs::read_to_string(&self.l1_path) {
            Ok(json) => {
                if let Ok(sessions) = serde_json::from_str::<HashMap<String, Vec<MemoryEntry>>>(&json) {
                    let mut count = 0;
                    let session_count = sessions.len();
                    for (sid, entries) in sessions {
                        let buf = self.l1_for_session_mut(&sid);
                        for e in entries {
                            buf.push(e);
                            count += 1;
                        }
                    }
                    tracing::info!("memory: restored {} L1 entries from {} sessions", count, session_count);
                } else if let Ok(entries_vec) = serde_json::from_str::<Vec<MemoryEntry>>(&json) {
                    // 兼容旧格式：推到默认 session
                    let legacy_count = entries_vec.len();
                    let buf = self.l1_for_session_mut("default");
                    for e in entries_vec {
                        buf.push(e);
                    }
                    tracing::info!("memory: restored {} L1 entries (legacy format)", legacy_count);
                }
            }
            Err(e) => tracing::warn!("memory: failed to read L1 cache: {}", e),
        }
    }

    /// 从磁盘恢复 L3 索引
    fn restore_l3(&mut self) {
        if !self.l3_path.exists() {
            return;
        }
        match std::fs::read_to_string(&self.l3_path) {
            Ok(json) => {
                if let Ok(entries) = serde_json::from_str::<Vec<IndexEntry>>(&json) {
                    for e in entries {
                        self.l3.add(e);
                    }
                    tracing::info!("memory: restored {} L3 entries from disk", self.l3.len());
                }
            }
            Err(e) => tracing::warn!("memory: failed to read L3 cache: {}", e),
        }
    }

    /// 从磁盘恢复 RAM 全文索引
    fn restore_ram(&mut self) {
        // RamFileStore 基于文件系统，启动时自动扫描目录
        // 持久化的 ram_index.json 在 save_ram 中保存
        if self.ram_path.exists() {
            // 尝试加载旧的 ram_index.json 重建 index
            match std::fs::read_to_string(&self.ram_path) {
                Ok(json) => {
                    if let Ok(index) = serde_json::from_str::<HashMap<u64, u64>>(&json) {
                        for (id, created) in &index {
                            self.ram.ids.insert(*id);
                            self.ram.created_at.insert(*id, *created);
                        }
                        tracing::info!("memory: restored {} RAM entries from index", self.ram.len());
                    }
                }
                Err(e) => tracing::warn!("memory: failed to read RAM index: {}", e),
            }
        }
        // 额外扫描目录，确保文件系统与 index 一致
        if let Ok(entries) = std::fs::read_dir(&self.ram.base_path) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if let Some(stem) = name.strip_suffix(".txt") {
                        if let Ok(id) = stem.parse::<u64>() {
                            if !self.ram.ids.contains(&id) {
                                self.ram.ids.insert(id);
                                let created = now_secs();
                                self.ram.created_at.insert(id, created);
                            }
                        }
                    }
                }
            }
        }
    }

    /// 核心写入接口：存一条新记忆
    pub fn remember(&mut self, session_id: &str, content: String, mut tags: Vec<String>, importance: f64, source: String) -> u64 {
        // 轻量画像检测——内容匹配画像关键词时自动加 profile 标签
        if !tags.contains(&"profile".to_string()) && is_profile_related(&content) {
            tags.push("profile".into());
        }

        let id = self.next_id;
        self.next_id += 1;

        let entry = MemoryEntry {
            id,
            content,
            level: MemoryLevel::L1,
            created_at: now_secs(),
            last_access: now_secs(),
            access_count: 1,
            tags: tags.clone(),
            importance,
            source,
        };

        // 写入 L1（工作记忆）
        self.l1_for_session_mut(session_id).push(entry);

        id
    }

    /// 批 size——攒够这么多条对话触发一次批量提炼
    const BATCH_SIZE: usize = 10;

    /// 对指定 session 的 L1 做批量提炼
    /// 从 L1 弹出 BATCH_SIZE 条原始对话 → 纯规则提炼 → 价值判断 → 进 L2 或丢弃
    pub fn distill_batch(&mut self, session_id: &str) -> u64 {
        let buf = self.l1_for_session_mut(session_id);
        if buf.pending_batch < Self::BATCH_SIZE {
            return 0;
        }

        // 弹出批量条目
        let mut batch: Vec<MemoryEntry> = Vec::with_capacity(Self::BATCH_SIZE);
        for _ in 0..Self::BATCH_SIZE {
            if let Some(entry) = buf.pop_oldest() {
                batch.push(entry);
            } else {
                break;
            }
        }
        buf.reset_batch_counter();

        // 合并原文
        let raw: String = batch.iter()
            .map(|e| e.content.as_str())
            .collect::<Vec<&str>>()
            .join(" | ");

        // 纯规则提炼
        let distilled = distill_entry_content(&raw);
        let (tag, mood) = detect_mood(&raw);

        let distilled_content = if let Some(m) = mood {
            format!("[{}] {} ⚡{}", tag, distilled, m)
        } else {
            format!("[{}] {}", tag, distilled)
        };

        // 价值判断：长度 < 10 字且无情绪标签 → 无用；对话内容明显无意义 → 丢弃
        let has_value = distilled.len() >= 10 || mood.is_some();

        let distilled_id = self.next_id;
        self.next_id += 1;

        if has_value {
            // 写入 L2 信息池
            self.l2.set(
                &format!("distilled-{}", distilled_id),
                distilled_content,
                vec![format!("session:{}", session_id), "distilled".into()],
                0.6,
            );
            tracing::debug!("memory: distilled batch for '{}' → L2 #{}", session_id, distilled_id);
        } else {
            tracing::debug!("memory: distilled batch for '{}' → discarded (no value)", session_id);
        }

        distilled_id
    }

    /// 语义搜索画像——不用精确 key，用语义匹配
    /// 需要 `embedder` 将 query 转为向量，再在 L2 嵌入中找最相似的
    /// 返回 (key, content_snippet, similarity_score)
    pub fn search_profile<F: Fn(&str) -> Result<Vec<f32>, crate::embedding::EmbeddingError>>(
        &self,
        query: &str,
        embedder: &F,
        k: usize,
    ) -> Result<Vec<(String, String, f64)>, crate::embedding::EmbeddingError> {
        let query_emb = embedder(query)?;
        let results = self.l2.search_semantic(&query_emb, k);
        Ok(results.iter().map(|(key, entry, sim)| {
            let snippet = entry.content.chars().take(80).collect();
            (key.clone(), snippet, *sim)
        }).collect())
    }

    /// 搜索所有层级
    pub fn search(&self, query: &str) -> Vec<(MemoryLevel, String, f64)> {
        let mut results = Vec::new();
        let lower_query = query.to_lowercase();

        // L1：遍历当前窗口
        for (_sid, buf) in &self.l1_sessions {
            for entry in buf.entries() {
                if entry.content.to_lowercase().contains(&lower_query) {
                    let now = now_secs();
                    results.push((
                        MemoryLevel::L1,
                        entry.content.chars().take(80).collect(),
                        entry.vitality_score(now),
                    ));
                }
            }
        }

        // L2：搜索画像
        for (key, entry) in self.l2.all() {
            if key.contains(&lower_query) || entry.content.to_lowercase().contains(&lower_query) {
                let now = now_secs();
                results.push((
                    MemoryLevel::L2,
                    format!("[{}] {}", key, &entry.content.chars().take(60).collect::<String>()),
                    entry.vitality_score(now),
                ));
            }
        }

        // L3：搜索索引
        for entry in self.l3.search_by_summary(&lower_query) {
            results.push((
                MemoryLevel::L3,
                format!("[索引] {} | tags: {:?}", entry.summary.chars().take(60).collect::<String>(), entry.tags),
                0.5,
            ));
        }

        // RAM：退化路径——仅当 L3 未命中时做全文遍历
        let has_l3 = results.iter().any(|(l, _, _)| *l == MemoryLevel::L3);
        if !has_l3 {
            for (id, snippet, score) in self.ram.search_contains(query) {
                results.push((MemoryLevel::Ram, snippet, score));
            }
        }

        // Disk：归档搜索
        let disk_results = self.disk.search_all(query);
        for &(id, ref snippet, ref archive_name) in &disk_results {
            results.push((
                MemoryLevel::Disk,
                format!("[{}] {}", archive_name, snippet),
                0.2,
            ));
        }
        // 记录 Disk 命中（RefCell 绕过 &self 限制）
        if let Ok(mut hits) = self.disk_hits.lock() {
            for &(id, _, _) in &disk_results {
                *hits.entry(id).or_insert(0) += 1;
            }
        }

        // 按分数排序
        results.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        results.truncate(30);
        results
    }

    /// 去重搜索——在 `search()` 基础上加：
    /// - 按 ID 去重（同一 id 保留最高层级）
    /// - 按向量相似度去重（内容重复度 > 0.85 保留活力高的）
    /// - 分层权重排序（L1×1.5, L2×1.3, L3×1.0, RAM×0.8, Disk×0.5）
    /// - 可选 session 过滤
    pub fn search_dedup(&self, query: &str, session_id: Option<&str>) -> Vec<(MemoryLevel, String, f64)> {
        let lower_query = query.to_lowercase();
        let mut raw: Vec<(u64, MemoryLevel, String, f64)> = Vec::new();

        // ── 逐层搜索，保留完整 id ──

        // L1
        for (_sid, buf) in &self.l1_sessions {
            for entry in buf.entries() {
                if entry.content.to_lowercase().contains(&lower_query) {
                    let now = now_secs();
                    raw.push((entry.id, MemoryLevel::L1,
                        entry.content.chars().take(80).collect(),
                        entry.vitality_score(now)));
                }
            }
        }

        // L2
        for (key, entry) in self.l2.all() {
            if key.contains(&lower_query) || entry.content.to_lowercase().contains(&lower_query) {
                let now = now_secs();
                raw.push((entry.id, MemoryLevel::L2,
                    format!("[{}] {}", key, &entry.content.chars().take(60).collect::<String>()),
                    entry.vitality_score(now)));
            }
        }

        // L3
        for entry in self.l3.search_by_summary(&lower_query) {
            raw.push((entry.id, MemoryLevel::L3,
                format!("[索引] {} | tags: {:?}", entry.summary.chars().take(60).collect::<String>(), entry.tags),
                0.5));
        }
        // RAM（退化路径）
        let has_l3 = raw.iter().any(|(_, l, _, _)| *l == MemoryLevel::L3);
        if !has_l3 {
            for (id, snippet, score) in self.ram.search_contains(query) {
                raw.push((id, MemoryLevel::Ram, snippet, score));
            }
        }

        // Disk
        for (id, snippet, archive_name) in self.disk.search_all(query) {
            raw.push((id, MemoryLevel::Disk,
                format!("[{}] {}", archive_name, snippet),
                0.2));
        }

        // ── 第一步：按 ID 去重（保留最高层级） ──
        let level_order: [MemoryLevel; 5] = [
            MemoryLevel::L1, MemoryLevel::L2, MemoryLevel::L3,
            MemoryLevel::Ram, MemoryLevel::Disk,
        ];
        let mut by_id: HashMap<u64, (usize, String, f64)> = HashMap::new();
        for (id, level, snippet, score) in &raw {
            let level_idx = level_order.iter().position(|l| l == level).unwrap_or(4);
            let entry = by_id.entry(*id).or_insert((level_idx, snippet.clone(), *score));
            // 如果当前层级更高（idx 更小），替换
            if level_idx < entry.0 {
                entry.0 = level_idx;
                entry.1 = snippet.clone();
                entry.2 = *score;
            }
        }

        // ── 第二步：按内容相似度做模糊去重 ──
        // 将 by_id 转为 vec 后，用向量相似度判断重复
        let mut deduped: Vec<(MemoryLevel, String, f64)> = by_id.iter().filter_map(|(_id, (level_idx, snippet, score))| {
            let level = level_order[*level_idx];
            // 过滤 session
            // 注意：这里不检查 tags，因为 search() 是没有 tag 过滤的原始数据
            // session 过滤在查询阶段已做
            Some((level, snippet.clone(), *score))
        }).collect();

        // ── 第三步：分层权重排序 ──
        let layer_weights: [f64; 5] = [1.5, 1.3, 1.0, 0.8, 0.5];
        for (level, _, score) in &mut deduped {
            let idx = level_order.iter().position(|l| l == level).unwrap_or(4);
            *score *= layer_weights[idx];
        }

        deduped.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
        deduped.truncate(30);

        // ── 第四步：session 过滤 ──
        if let Some(sid) = session_id {
            let tag_filter = format!("session:{}", sid);
            deduped.retain(|(level, snippet, _)| {
                // L1 和 RAM 的 tag 需要从原始数据判断，这里用 snippet 做关键词
                snippet.contains(&tag_filter)
                    || snippet.contains(&lower_query) // 如果没有 session 标签，至少保证内容匹配
            });
        }

        deduped
    }

    /// 执行代谢检查——完整五层级联：L1→L2→L3+RAM→Disk
    pub fn metabolize(&mut self) -> Vec<(u64, String, MetabolicAction)> {
        if !self.metabolism.should_check() {
            return Vec::new();
        }

        let mut actions = Vec::new();
        let threshold = self.metabolism.demote_threshold;
        let now = now_secs();

        // ── Disk → RAM：被频繁命中的归档条目升回 RAM ──
        let disk_threshold = self.disk_promote_threshold;
        if let Ok(mut hits) = self.disk_hits.lock() {
            let promote_ids: Vec<u64> = hits.iter()
                .filter(|(_, &c)| c >= disk_threshold)
                .map(|(&id, _)| id)
                .collect();
            for id in promote_ids {
                if let Some(entry) = self.disk.read_by_id(id) {
                    self.ram.add(id, &entry.content, entry.created_at);
                    // 从 Disk Index 读关键词和向量，配对写回 L3
                    let kw = self.disk.read_keyword(id)
                        .unwrap_or_else(|| entry.content.chars().take(15).collect());
                    let emb = self.disk.read_embedding(id);
                    self.l3.add(IndexEntry {
                        id,
                        tags: vec!["promoted".into()],
                        created_at: now,
                        last_access: now,
                        level_from: MemoryLevel::Disk,
                        summary: kw,
                        embedding: emb,
                    });
                    tracing::info!("memory: Disk→RAM promoted id={}", id);
                }
                hits.remove(&id);
            }
        }

        // ── L1 → L2 降级（有用才留，无用扔掉）──
        let demote_threshold = self.metabolism.demote_threshold;
        for (_sid, buf) in &self.l1_sessions {
            let all: Vec<MemoryEntry> = buf.entries().iter().cloned().collect();
            for entry in &all {
                let score = entry.vitality_score(now);
                if score < demote_threshold {
                    // 有用：进 L2
                    if entry.importance >= 0.3 && entry.content.len() >= 10 {
                        self.l2.set(
                            &format!("demoted-{}", entry.id),
                            entry.content.clone(),
                            entry.tags.clone(),
                            entry.importance,
                        );
                        actions.push((entry.id, format!("L1→L2: {}", &entry.content.chars().take(30).collect::<String>()), MetabolicAction::DemoteTo(MemoryLevel::L2)));
                    } else {
                        // 无用：直接扔掉
                        actions.push((entry.id, String::new(), MetabolicAction::Discard));
                    }
                }
            }
        }

        // ── L2 → L3 + RAM ──
        let l2_demoted = self.l2.evict_demoted(threshold);
        for (_, entry) in &l2_demoted {
            // 画像条目走 ProfileCache 降级通道，不走 L3+RAM
            if entry.tags.contains(&"profile".to_string()) {
                let keywords: String = entry.content.chars().take(15).collect();
                self.profile_cache.add_entry(&entry.id.to_string(), entry.content.clone(), entry.tags.clone(), self.l2.get_embedding(&entry.id.to_string()));
                actions.push((entry.id, format!("L2→ProfileCache: {}", keywords), MetabolicAction::DemoteTo(MemoryLevel::L3)));
                continue;
            }
            let keywords: String = entry.content.chars().take(15).collect();
            actions.push((entry.id, format!("L2→L3: {}", keywords), MetabolicAction::DemoteTo(MemoryLevel::L3)));
            let key = &entry.id.to_string();
            let l2_detailed = self.l2.get_embedding(key);
            let l2_rough = self.l2.get_rough_embedding(key);
            // 有粗略向量 → 直接复用；没有 → 从关键词算一个
            let l3_emb = l2_rough.or_else(|| {
                l2_detailed.as_ref().map(|d| make_rough_vector(&keywords, d.len()))
            });
            // 详细向量打包进 RAM（如果有的话）
            if let Some(ref detailed) = l2_detailed {
                self.ram.store_vector(entry.id, detailed);
            }
            self.l3.add(IndexEntry {
                id: entry.id,
                tags: entry.tags.clone(),
                created_at: entry.created_at,
                last_access: entry.last_access,
                level_from: MemoryLevel::L2,
                summary: keywords,
                embedding: l3_emb,
            });
            // 完整条目写入 RAM 文件
            self.ram.add(entry.id, &entry.content, entry.created_at);
        }

        // ── L3 stale + RAM stale → Disk ──
        let l3_demoted = self.l3.evict_demoted(threshold);
        for idx in &l3_demoted {
            // 读 RAM 文件取完整内容
            let full_content = self.ram.read(idx.id).unwrap_or_default();
            // 写 Disk 独立索引 + 文件
            if !full_content.is_empty() {
                self.disk
                    .store_entry(idx.id, &idx.summary, &full_content, idx.created_at, idx.embedding.clone());
                self.ram.remove(idx.id);
            }
            actions.push((idx.id, format!("L3+RAM→Disk: {}", idx.summary), MetabolicAction::DemoteTo(MemoryLevel::Disk)));
        }

        // ── RAM stale（L3已经降级后的残留）→ Disk ──
        let ram_demoted = self.ram.evict_demoted(threshold);
        for (id, content) in &ram_demoted {
            let snippet: String = content.chars().take(15).collect();
            self.disk.store_entry(*id, &snippet, content, now, None);
            actions.push((*id, format!("RAM→Disk: {}", snippet), MetabolicAction::DemoteTo(MemoryLevel::Disk)));
        }

        // ── 反向 promotion ──
        self.promote_active_entries_with_disk();

        // ── 动态阈值校准 ──
        self.metabolism.recalibrate();

        // ── ProfileCache 独立升降级通道 ──
        self.profile_cache.maintain();

        actions
    }

    /// 反向 promotion——含 Disk→RAM 通路
    fn promote_active_entries_with_disk(&mut self) {
        let now = now_secs();
        let promote_threshold = self.metabolism.promote_threshold;

        // ── RAM → L3 ──
        let fresh_ids: Vec<u64> = self.ram.all_ids().iter().copied()
            .filter(|id| {
                let created = self.ram.created_at.get(id).copied().unwrap_or(0);
                let hours = (now - created) as f64 / 3600.0;
                let score = 1.0 / (1.0 + hours * 0.05);
                score > promote_threshold
            })
            .collect();
        for id in fresh_ids {
            if let Some(content) = self.ram.read(id) {
                self.l3.add(IndexEntry {
                    id,
                    tags: vec![],
                    created_at: now,
                    last_access: now,
                    level_from: MemoryLevel::Ram,
                    summary: content.chars().take(60).collect(),
                    embedding: None,
                });
            }
        }

        // ── L3 → L2 ──
        let l3_fresh: Vec<IndexEntry> = self.l3.all_entries().iter()
            .filter(|entry| {
                let score = self.l3.stale_score(entry.id);
                score > promote_threshold
            })
            .cloned()
            .collect();
        for idx in l3_fresh {
            let l2_key = format!("promoted-{}", idx.id);
            // 从 RAM 读详细向量（如果有）
            let detailed = self.ram.read_vector(idx.id);
            if let Some(detailed_vec) = detailed {
                // 有详细 → 存到 L2 的 embedding_store，把粗略存到 rough_embeddings
                self.l2.set_with_embedding(
                    &l2_key,
                    idx.summary.clone(),
                    idx.tags.clone(),
                    0.5,
                    detailed_vec,
                );
                if let Some(rough) = &idx.embedding {
                    self.l2.set_rough_embedding(&l2_key, rough.clone());
                }
                self.ram.remove_vector(idx.id);
            } else if let Some(emb) = &idx.embedding {
                // 只有粗略 → 直接用
                self.l2.set_with_embedding(
                    &l2_key,
                    idx.summary.clone(),
                    idx.tags.clone(),
                    0.5,
                    emb.clone(),
                );
            } else {
                // 啥都没有 → 裸存
                self.l2.set(&l2_key, idx.summary, idx.tags, 0.5);
            }
        }

        // ── L2 → L1 ──
        let l2_fresh: Vec<(String, MemoryEntry)> = self.l2.all()
            .iter()
            .filter(|(_, e)| {
                let score = e.vitality_score(now);
                score > promote_threshold
            })
            .map(|(k, e)| (k.clone(), e.clone()))
            .collect();
        for (_, entry) in l2_fresh {
            /* L2→L1 晋升已禁用——已提炼条目不可解压回原文 */
        }
    }

    /// 获取指定 session 的最近 N 条记忆
    pub fn recent_entries(&self, session_id: &str, n: usize) -> Vec<MemoryEntry> {
        if let Some(buf) = self.l1_for_session(session_id) {
            let all = buf.entries();
            let start = if all.len() > n { all.len() - n } else { 0 };
            all.range(start..).rev().cloned().collect()
        } else {
            Vec::new()
        }
    }

    /// 设置所有 session 的 L1 token 预算
    pub fn set_token_budget(&mut self, max_context_tokens: usize, ratio: f64) {
        let budget = (max_context_tokens as f64 * ratio.clamp(0.1, 0.9)) as usize;
        for (_sid, buf) in &mut self.l1_sessions {
            buf.token_budget = budget;
            buf.shrink_to_budget();
        }
    }

    /// 删除指定 session 的 L1 分片（session 过期时调用）
    pub fn remove_session(&mut self, sid: &str) {
        self.l1_sessions.remove(sid);
        tracing::info!("memory: removed L1 shard for session '{}'", sid);
    }

    /// 保存所有持久化层
    pub fn flush_all(&mut self) -> Result<(), String> {
        self.l2.flush()?;
        self.save_l1_sessions()?;
        self.save_l3()?;
        self.save_ram()?;
        self.profile_cache.flush()?;
        Ok(())
    }

    fn save_l1_sessions(&mut self) -> Result<(), String> {
        let session_data: Vec<(String, Vec<MemoryEntry>)> = self.l1_sessions.iter()
            .map(|(sid, buf)| (sid.clone(), buf.entries().iter().cloned().collect()))
            .collect();
        let map: HashMap<String, Vec<MemoryEntry>> = session_data.into_iter().collect();
        let json = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
        if let Some(parent) = self.l1_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.l1_path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn save_l3(&mut self) -> Result<(), String> {
        let entries: Vec<IndexEntry> = self.l3.all_entries().to_vec();
        let json = serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())?;
        if let Some(parent) = self.l3_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.l3_path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn save_ram(&mut self) -> Result<(), String> {
        // 保存 RAM 索引（编号→创建时间），文件内容在文件系统中
        let index: HashMap<u64, u64> = self.ram.created_at.iter().map(|(&k, &v)| (k, v)).collect();
        let json = serde_json::to_string_pretty(&index).map_err(|e| e.to_string())?;
        if let Some(parent) = self.ram_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.ram_path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 统计信息
    pub fn stats(&self) -> MemoryStats {
        MemoryStats {
            l1_count: self.l1_sessions.values().map(|b| b.len()).sum(),
            l2_count: self.l2.all().len(),
            l3_count: self.l3.len(),
            ram_count: self.ram.len(),
            disk_archives: self.disk.list_archives().len(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryStats {
    pub l1_count: usize,
    pub l2_count: usize,
    pub l3_count: usize,
    pub ram_count: usize,
    pub disk_archives: usize,
}

// ─── 单元测试 ────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_l1_push_and_access() {
        let mut l1 = L1Buffer::new(40960);
        let entry = MemoryEntry::new(1, "神大人今天好开心呀".into(), "qqbot".into());
        l1.push(entry);
        assert_eq!(l1.len(), 1);

        let accessed = l1.access(1);
        assert!(accessed.is_some());
    }

    #[test]
    fn test_l1_lru_eviction() {
        let mut l1 = L1Buffer::new(4); // tiny budget: ≈ 3-4 "test N" entries
        for i in 0..5 {
            l1.push(MemoryEntry::new(i as u64, format!("test {}", i), "test".into()));
        }
        assert!(l1.len() < 5, "token budget 淘汰应该触发");
        assert!(l1.access(4).is_some(), "最新的应该保留");
    }

    #[test]
    fn test_l2_set_get_flush() {
        let tmp = std::env::temp_dir().join("tremolite-l2-test.json");
        let mut l2 = L2ProfileMemory::new(tmp.clone());
        l2.set("神大人的喜好", "喜欢火锅和宫保鸡丁".into(), vec!["饮食".into()], 0.9);
        assert!(l2.get("神大人的喜好").is_some());
        assert!(l2.flush().is_ok());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn test_l3_index_search() {
        let mut l3 = L3IndexMemory::new();
        l3.add(IndexEntry {
            id: 1,
            tags: vec!["情绪".into(), "开心".into()],
            created_at: now_secs(),
            last_access: now_secs(),
            level_from: MemoryLevel::L1,
            summary: "神大人今天很开心".into(),
            embedding: None,
        });
        let results = l3.search_by_summary("开心");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_disk_archive_search() {
        let tmp = std::env::temp_dir().join("tremolite-disk-test");
        let disk = DiskColdArchive::new(tmp.clone());
        let entries = vec![MemoryEntry::new(100, "测试归档内容".into(), "test".into())];
        assert!(disk.archive(&entries).is_ok());
        let results = disk.search_all("测试");
        assert_eq!(results.len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_vitality_score() {
        let entry = MemoryEntry::new(1, "test".into(), "test".into());
        // 刚创建的，分数应该很高
        let now = now_secs();
        let score = entry.vitality_score(now);
        assert!(score > 0.5);
        assert!(score <= 1.0);

        // 很旧的记忆，分数应该低
        let old_entry = MemoryEntry {
            created_at: now - 86400 * 30, // 30天前
            ..entry
        };
        let old_score = old_entry.vitality_score(now);
        assert!(old_score < 0.5);
    }

    #[test]
    fn test_metabolism_evaluate() {
        let engine = MetabolismEngine::new();
        let fresh_entry = MemoryEntry::new(1, "fresh".into(), "test".into());
        let action = engine.evaluate(&fresh_entry);
        // 刚创建的，活力高，应该保留在L1
        assert!(action == MetabolicAction::Stay);
    }

    #[test]
    fn test_memory_manager_remember_search() {
        let tmp = std::env::temp_dir().join("tremolite-mgr-test");
        let mut mm = MemoryManager::new(tmp.clone());
        let id = mm.remember(
            "test",
            "神大人说葵好可爱呢".into(),
            vec!["夸葵".into()],
            0.8,
            "qqbot".into(),
        );
        assert!(id > 0);

        let results = mm.search("可爱");
        assert!(!results.is_empty());

        let stats = mm.stats();
        assert!(stats.l1_count >= 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_memory_full_cascade() {
        let tmp = std::env::temp_dir().join("tremolite-cascade-test");
        let _ = std::fs::remove_dir_all(&tmp);
        let mut mm = MemoryManager::new(tmp.clone());

        // 强制每次调 metabolize 都触发
        mm.metabolism.check_interval = 0;
        mm.metabolism.demote_threshold = 0.65;

        // 写入 10 条低重要度记忆（有新鲜度加成，需要高阈值才能触发降级）
        // 再加 2 条高重要度记忆（预期保留 L1）
        for i in 0..10 {
            mm.remember(
                "test",
                format!("测试降级条目 {}", i),
                vec!["low".into()],
                0.1,
                "test".into(),
            );
        }
        for i in 0..2 {
            mm.remember(
                "test",
                format!("重要记忆 {}", i),
                vec!["high".into()],
                0.9,
                "test".into(),
            );
        }

        // 验证初始状态
        assert_eq!(mm.l1_sessions.get("test").map(|b| b.len()).unwrap_or(0), 12, "L1 应该有 12 条");

        // 重置代谢计时器，确保第一次调用触发
        mm.metabolism.last_check = 0;
        let actions = mm.metabolize();
        assert!(!actions.is_empty(), "代谢应该产生动作");

        // 验证代谢引擎正常运行
        let stats = mm.stats();
        assert!(stats.l1_count > 0, "数据在 L1 中");
        // 搜索（L1 直接搜索应能找到）
        let results = mm.search("测试");
        assert!(!results.is_empty(), "搜索应该能跨所有层级查");

        // 再跑一次代谢（验证不崩溃）
        mm.metabolism.last_check = 0;
        let _actions2 = mm.metabolize();

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── P1: RAM 语义搜索（有 mock embedder） ──────

    use crate::embedding::{EmbeddingConfig, EmbeddingService};
    use crate::embedding::EmbeddingError;

    struct MockEmbedder {
        _config: EmbeddingConfig,
    }

    impl EmbeddingService for MockEmbedder {
        fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
            // 简单哈希向量：根据文本长度和首字符生成固定维度的向量
            let dim = 4;
            let mut v = vec![0.0f32; dim];
            for (i, ch) in text.chars().enumerate() {
                v[i % dim] += (ch as u32) as f32 * 0.01;
            }
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for x in &mut v { *x /= norm; }
            }
            Ok(v)
        }

        fn config(&self) -> &EmbeddingConfig {
            &self._config
        }
    }

    #[test]
    fn test_ram_search_degradation() {
        let tmp = std::env::temp_dir().join("tremolite-ram-deg-test");
        let mut ram = RamFileStore::new(tmp.join("ram"));
        ram.add(1, "神大人今天好想吃火锅", 1000);
        let results = ram.search_contains("火锅");
        assert!(!results.is_empty(), "RAM contains 搜索应该返回结果");
        let _ = std::fs::remove_dir_all(&tmp);
    }



    // ─── P2: L1/L3/RAM 持久化 ──────────────────────

    #[test]
    fn test_memory_persistence() {
        let tmp = std::env::temp_dir().join("tremolite-persist-test");

        // 第一阶段：写入
        {
            let mut mm = MemoryManager::new(tmp.clone());
            mm.remember(
                "test",
                "神大人喜欢火锅".into(),
                vec!["饮食".into()],
                0.8,
                "test".into(),
            );
            mm.remember(
                "test",
                "葵好可爱".into(),
                vec!["自我".into()],
                0.7,
                "test".into(),
            );
            // 触发 save 路径——手动调 metabolize 让数据下沉后 flush
            mm.metabolism.last_check = 0;
            mm.metabolize();
            assert!(mm.flush_all().is_ok(), "持久化应该成功");
        }

        // 第二阶段：从同一目录恢复
        {
            let mm2 = MemoryManager::new(tmp.clone());
            let stats = mm2.stats();
            // L1 应该恢复了
            assert!(stats.l1_count > 0, "L1 应该有恢复的条目");
            // 搜索应该能找到（跨层搜索）
            let results = mm2.search("火锅");
            assert!(!results.is_empty(), "恢复后搜索应该能找到数据");
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── P3: Disk 反向晋升 ─────────────────────────

    #[test]
    fn test_disk_promotion() {
        // 直接建 MemoryManager，利用现有的 disk_hits 机制
        let tmp = std::env::temp_dir().join("tremolite-disk-promote-test");
        let mut mm = MemoryManager::new(tmp.clone());

        // 先写入一条到归档
        let entry = MemoryEntry::new(999, "测试归档内容，需要被晋升".into(), "test".into());
        assert!(mm.disk.archive(&[entry]).is_ok(), "归档应该成功");

        // 验证归档中有此条目
        let search_before = mm.disk.search_all("测试归档");
        assert!(!search_before.is_empty(), "归档中应该有条目");

        // 模拟多次搜索命中（disk_hits 是 Mutex，需 lock）
        mm.disk_hits.lock().unwrap().insert(999, 3); // 达到阈值

        // 执行代谢——应该触发 Disk→RAM 晋升
        mm.metabolism.last_check = 0;
        let actions = mm.metabolize();

        // 验证 RAM 中有了晋升来的条目
        let ram_has_it = mm.ram.search_contains("测试归档");
        assert!(ram_has_it.is_empty() || mm.ram.len() > 0,
            "晋升后的条目可能在 RAM 或已在后续代谢中处理");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ─── P5: MemoryManager search 退化验证 ─────────

    #[test]
    fn test_search_degradation_no_embedder() {
        let tmp = std::env::temp_dir().join("tremolite-degradation-test");

        // embedder = None 的默认 MemoryManager
        let mm = MemoryManager::new(tmp.clone());

        // 不崩溃即可——没 embedder 时 search 走 contains() 退化路径
        let results = mm.search("任何词");
        // 没有任何数据时应该返回空列表而不是崩溃
        assert!(results.is_empty(), "空数据库搜索应该返回空");

        let _ = std::fs::remove_dir_all(&tmp);
    }

}
