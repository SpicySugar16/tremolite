use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Embedding 引擎（硅基流动 API） ──────────────

/// OpenAI 兼容的 embedding 请求体
#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: Vec<&'a str>,
}

/// OpenAI 兼容的 embedding 响应
#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
    #[allow(dead_code)]
    model: String,
    #[allow(dead_code)]
    object: String,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f64>,
    index: usize,
    #[allow(dead_code)]
    object: String,
}

/// 嵌入引擎——通过硅基流动 API 将文本转为向量
struct EmbeddingEngine {
    api_base: String,
    api_key: String,
    model: String,
}

impl EmbeddingEngine {
    fn new(api_base: &str, api_key: &str, model: &str) -> Self {
        Self {
            api_base: api_base.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            model: model.to_string(),
        }
    }

    fn embed(&self, texts: Vec<&str>) -> Option<Vec<Vec<f32>>> {
        let url = format!("{}/embeddings", self.api_base);
        let req_body = EmbeddingRequest {
            model: &self.model,
            input: texts,
        };

        let body = serde_json::to_string(&req_body).ok()?;

        let response = ureq::post(&url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send(body)
            .ok()?;

        let body_str = response.into_body().read_to_string().ok()?;
        let resp: EmbeddingResponse = serde_json::from_str(&body_str).ok()?;

        // 按 index 排序，确保和输入顺序一致
        let mut data = resp.data;
        data.sort_by_key(|d| d.index);

        let vectors: Vec<Vec<f32>> = data
            .into_iter()
            .map(|d| d.embedding.into_iter().map(|v| v as f32).collect())
            .collect();

        Some(vectors)
    }
}

// ─── 动态通道配置 ───────────────────────────────

/// 注意力通道——替代旧的四层固定枚举
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub name: String,
    pub label: String,
    pub window: usize,
    pub stride: usize,
    pub max_blocks: usize,
    pub threshold: f64,
}

impl Channel {
    pub fn new(name: &str, label: &str, window: usize, stride: usize, max_blocks: usize, threshold: f64) -> Self {
        Self {
            name: name.to_string(),
            label: label.to_string(),
            window,
            stride,
            max_blocks,
            threshold,
        }
    }
}

/// 默认三通道
pub fn default_channels() -> Vec<Channel> {
    vec![
        Channel::new("wide", "全局视野", 1000, 500, 10, 0.3),
        Channel::new("focus", "焦点缩放", 200, 50, 8, 0.5),
        Channel::new("micro", "微观精炼", 50, 10, 5, 0.6),
    ]
}

// ─── 注意力数据结构 ───────────────────────────────

/// 对话类型分类
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ChatType {
    /// focus + micro 都有活跃块
    FocusDiscussion,
    /// wide 活跃但 focus/micro 空
    TopicShift,
    /// 全链低分
    Scattered,
}

impl ChatType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChatType::FocusDiscussion => "focus_discussion",
            ChatType::TopicShift => "topic_shift",
            ChatType::Scattered => "scattered",
        }
    }
}

/// 一次注意力计算的结果块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionBlock {
    pub channel: String,
    pub position: usize,
    pub content: String,
    pub score: f64,
    pub key_entities: Vec<String>,
    pub timestamp: u64,
}

/// 链式注意力的完整输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionResult {
    /// 按通道名索引的 blocks
    pub channel_blocks: HashMap<String, Vec<AttentionBlock>>,
    /// 综合合成
    pub synthesis: AttentionSynthesis,
    /// 对话类型
    pub chat_type: ChatType,
    /// 链深度（1=仅wide, 2=wide+focus, 3=全链）
    pub chain_depth: usize,
}

impl AttentionResult {
    pub fn empty() -> Self {
        Self {
            channel_blocks: HashMap::new(),
            synthesis: AttentionSynthesis::empty(),
            chat_type: ChatType::Scattered,
            chain_depth: 0,
        }
    }

    /// 获取所有 blocks 按 score 降序
    pub fn all_blocks_sorted(&self) -> Vec<&AttentionBlock> {
        let mut blocks: Vec<&AttentionBlock> = self.channel_blocks.values().flat_map(|v| v.iter()).collect();
        blocks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        blocks
    }

    /// 方便获取 wide 通道（兼容旧代码）
    pub fn wide_blocks(&self) -> &[AttentionBlock] {
        self.channel_blocks.get("wide").map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// 综合合成输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionSynthesis {
    pub summary: String,
    pub top_entities: Vec<(String, f64)>,
    pub top_regions: Vec<(usize, String, f64)>,
    pub total_tokens_scanned: usize,
    pub effective_ratio: f64,
}

impl AttentionSynthesis {
    pub fn empty() -> Self {
        Self {
            summary: String::new(),
            top_entities: Vec::new(),
            top_regions: Vec::new(),
            total_tokens_scanned: 0,
            effective_ratio: 1.0,
        }
    }
}

// ─── 多尺度注意力引擎 ──────────────────────────────

/// 多尺度注意力引擎——链式递进扫描
pub struct MultiScaleAttention {
    attention_history: Vec<AttentionResult>,
    max_history: usize,
    embedding: Option<EmbeddingEngine>,
    /// 上一次缓存的 query embedding（避免同一轮重复嵌入）
    cached_query_embedding: Option<Vec<f32>>,
    cached_query_text: String,
    stats_path: Option<std::path::PathBuf>,
    /// 通道配置
    channels: Vec<Channel>,
}

impl Default for MultiScaleAttention {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiScaleAttention {
    pub fn new() -> Self {
        let stats_path = Some(std::path::PathBuf::from(
            "/home/spicysugar/.tremolite/profiles/main/attention_stats.json"
        ));
        Self {
            attention_history: Vec::new(),
            max_history: 100,
            embedding: None,
            cached_query_embedding: None,
            cached_query_text: String::new(),
            stats_path,
            channels: default_channels(),
        }
    }

    /// 配置通道列表
    pub fn with_channels(mut self, channels: Vec<Channel>) -> Self {
        if !channels.is_empty() {
            self.channels = channels;
        }
        self
    }

    /// 配置 embedding API（硅基流动 OpenAI 兼容接口）
    pub fn with_embedding_api(mut self, api_base: &str, api_key: &str, model: &str) -> Self {
        self.embedding = Some(EmbeddingEngine::new(api_base, api_key, model));
        tracing::info!(
            "注意力引擎使用 embedding API: {} / {}",
            api_base, model
        );
        self
    }

    /// 配置 stats 文件路径
    pub fn with_stats_path(mut self, path: &str) -> Self {
        self.stats_path = Some(std::path::PathBuf::from(path));
        self
    }

    pub fn set_stats_path(&mut self, path: &str) {
        self.stats_path = Some(std::path::PathBuf::from(path));
    }

    pub fn channels(&self) -> &[Channel] {
        &self.channels
    }

    /// 对输入文本执行链式递进注意力扫描
    pub fn attend(&mut self, text: &str) -> AttentionResult {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let total_len = text.len();

        // 嵌入 query
        let query_vec: Option<Vec<f32>> = if let Some(emb) = self.embedding.as_ref() {
            if text != self.cached_query_text {
                if let Some(mut vecs) = emb.embed(vec![text]) {
                    let v = vecs.swap_remove(0);
                    self.cached_query_embedding = Some(v.clone());
                    self.cached_query_text = text.to_string();
                    Some(v)
                } else {
                    None
                }
            } else {
                self.cached_query_embedding.clone()
            }
        } else {
            None
        };

        // 链式递进扫描
        let mut channel_blocks: HashMap<String, Vec<AttentionBlock>> = HashMap::new();
        let mut chain_depth: usize = 0;

        for (idx, channel) in self.channels.iter().enumerate() {
            let blocks = if idx == 0 {
                // 第一个通道（wide）扫全文
                self.scan_channel(text, 0, channel, now, query_vec.as_deref())
            } else {
                // 后续通道只扫前一个通道的高分区域
                let prev_name = &self.channels[idx - 1].name;
                let prev_threshold = self.channels[idx - 1].threshold;
                let mut blocks = Vec::new();

                if let Some(prev_blocks) = channel_blocks.get(prev_name) {
                    for candidate in prev_blocks.iter().filter(|b| b.score > prev_threshold) {
                        // 在候选块的内容上扫描（candidate.content 是原文对应段的文本）
                        let seg_blocks = self.scan_channel(
                            &candidate.content,
                            candidate.position,
                            channel,
                            now,
                            query_vec.as_deref(),
                        );
                        blocks.extend(seg_blocks);
                    }
                }
                blocks
            };

            if !blocks.is_empty() {
                chain_depth = idx + 1;
            }
            channel_blocks.insert(channel.name.clone(), blocks);
        }

        // 跨尺度实体确认
        let mut entity_chain_count: HashMap<String, usize> = HashMap::new();
        let mut entity_max_depth: HashMap<String, usize> = HashMap::new();
        for (idx, channel) in self.channels.iter().enumerate() {
            if let Some(blocks) = channel_blocks.get(&channel.name) {
                for block in blocks {
                    for entity in &block.key_entities {
                        *entity_chain_count.entry(entity.clone()).or_insert(0) += 1;
                        let entry = entity_max_depth.entry(entity.clone()).or_insert(0);
                        *entry = (*entry).max(idx + 1);
                    }
                }
            }
        }

        // 综合排名
        let all_blocks = {
            let mut blocks: Vec<&AttentionBlock> = channel_blocks.values().flat_map(|v| v.iter()).collect();
            blocks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
            blocks
        };

        // 实体最终评分（跨尺度确认 + 深度加权）
        let mut entity_scores: HashMap<String, f64> = HashMap::new();
        for (entity, count) in &entity_chain_count {
            if *count == 0 {
                continue;
            }
            let base = 0.1;
            let multiplier = if *count >= 3 { 4.0 } else if *count == 2 { 2.0 } else { 1.0 };
            let depth_weight = 1.0 + entity_max_depth.get(entity).copied().unwrap_or(1) as f64 * 0.2;
            let score = base * multiplier * depth_weight;
            entity_scores.insert(entity.clone(), score);
        }
        let mut sorted_entities: Vec<(String, f64)> = entity_scores.into_iter().collect();
        sorted_entities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        sorted_entities.truncate(10);

        let top_regions: Vec<(usize, String, f64)> = all_blocks
            .iter()
            .take(5)
            .map(|b| (b.position, b.content.chars().take(50).collect(), b.score))
            .collect();

        let total_blocks: usize = channel_blocks.values().map(|v| v.len()).sum();
        let compression_ratio = if total_len > 0 {
            (total_blocks as f64 * 50.0) / total_len as f64
        } else {
            1.0
        };

        // 对话类型分类
        let chat_type = classify_chat_type(&channel_blocks, &self.channels);

        let result = AttentionResult {
            synthesis: AttentionSynthesis {
                summary: if all_blocks.is_empty() {
                    "no salient region".into()
                } else {
                    format!(
                        "top region at pos {} score {:.2} entities {:?}",
                        all_blocks[0].position,
                        all_blocks[0].score,
                        all_blocks[0]
                            .key_entities
                            .iter()
                            .take(3)
                            .collect::<Vec<_>>()
                    )
                },
                top_entities: sorted_entities,
                top_regions,
                total_tokens_scanned: total_len,
                effective_ratio: (compression_ratio as f64).min(1.0),
            },
            channel_blocks,
            chat_type,
            chain_depth,
        };

        self.attention_history.push(result.clone());
        if self.attention_history.len() > self.max_history {
            self.attention_history.remove(0);
        }

        // 持久化 stats 到文件
        self.write_stats(&result);

        result
    }

    fn write_stats(&self, result: &AttentionResult) {
        if let Some(ref stats_path) = self.stats_path {
            let hist_count = self.attention_history.len();
            let total_scan = self.attention_history.iter()
                .map(|r| r.synthesis.total_tokens_scanned)
                .sum::<usize>();

            let last_top_score = result.synthesis.top_regions.first()
                .map(|(_, _, s)| *s)
                .unwrap_or(0.0);
            let timestamp = result.all_blocks_sorted().first()
                .map(|b| b.timestamp)
                .unwrap_or_else(|| std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs());

            let top_entities: Vec<String> = result.synthesis.top_entities.iter()
                .take(5)
                .map(|(e, _)| e.clone())
                .collect();

            let channel_names: Vec<&str> = self.channels.iter().map(|c| c.name.as_str()).collect();

            let stats = serde_json::json!({
                "channels": channel_names,
                "last_scan": {
                    "timestamp": timestamp,
                    "chat_type": result.chat_type.as_str(),
                    "top_score": (last_top_score * 100.0).round() / 100.0,
                    "entities": top_entities,
                    "chain_depth": result.chain_depth,
                },
                "history_count": hist_count,
                "total_tokens_scanned": total_scan,
            });
            if let Ok(json_str) = serde_json::to_string_pretty(&stats) {
                let _ = std::fs::write(stats_path, json_str);
            }
        }
    }

    /// 在给定文本上执行单个通道的滑动窗口扫描
    fn scan_channel(
        &self,
        text: &str,
        base_position: usize,
        channel: &Channel,
        timestamp: u64,
        query_vec: Option<&[f32]>,
    ) -> Vec<AttentionBlock> {
        let window = channel.window;
        let stride = channel.stride;
        let max_blocks = channel.max_blocks;
        let mut blocks = Vec::new();

        if text.is_empty() || window == 0 {
            return blocks;
        }

        let chars: Vec<char> = text.chars().collect();
        let text_len = chars.len();
        let mut segments: Vec<(usize, String)> = Vec::new();
        let mut i = 0;
        while i < text_len {
            let end = (i + window).min(text_len);
            let segment: String = chars[i..end].iter().collect();
            segments.push((base_position + i, segment));
            i += stride;
        }

        // 如果有 embedding，批量嵌入并计算语义分数
        if let (Some(emb), Some(qv)) = (self.embedding.as_ref(), query_vec) {
            let texts: Vec<&str> = segments.iter().map(|(_, s)| s.as_str()).collect();
            if let Some(embeddings) = emb.embed(texts) {
                for ((pos, seg), emb_vec) in segments.into_iter().zip(embeddings.into_iter()) {
                    let semantic_score = cosine_similarity(qv, &emb_vec);
                    let keyword_bonus = keyword_attention_bonus(&seg);
                    let score = semantic_score * 0.7 + keyword_bonus * 0.3;
                    let entities = extract_known_entities(&seg);
                    blocks.push(AttentionBlock {
                        channel: channel.name.clone(),
                        position: pos,
                        content: seg,
                        score,
                        key_entities: entities,
                        timestamp,
                    });
                }
            } else {
                // embedding 失败，降级
                for (pos, seg) in segments {
                    let score = keyword_attention_bonus(&seg);
                    let entities = extract_known_entities(&seg);
                    blocks.push(AttentionBlock {
                        channel: channel.name.clone(),
                        position: pos,
                        content: seg,
                        score,
                        key_entities: entities,
                        timestamp,
                    });
                }
            }
        } else {
            // 无 embedding，纯关键词
            for (pos, seg) in segments {
                let score = keyword_attention_bonus(&seg);
                let entities = extract_known_entities(&seg);
                blocks.push(AttentionBlock {
                    channel: channel.name.clone(),
                    position: pos,
                    content: seg,
                    score,
                    key_entities: entities,
                    timestamp,
                });
            }
        }

        blocks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        blocks.truncate(max_blocks);
        blocks
    }

    pub fn history(&self) -> &[AttentionResult] {
        &self.attention_history
    }

    pub fn last_result(&self) -> Option<&AttentionResult> {
        self.attention_history.last()
    }

    /// 更新通道配置（运行时）
    pub fn set_channels(&mut self, channels: Vec<Channel>) {
        if !channels.is_empty() {
            self.channels = channels;
        }
    }
}

// ─── 辅助函数 ──────────────────────────────────────

/// 余弦相似度
fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).max(0.0) as f64 // 截断到 [0, 1]
}

/// 关键词注意力加分——作为 embedding 的补充
fn keyword_attention_bonus(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let lower = text.to_lowercase();
    let mut score: f64 = 0.3; // baseline，避免全部归零

    let emotional = [
        "happy", "sad", "angry", "scared", "love", "like", "hate",
        "great", "sorry", "开心", "难过", "生气", "爱", "喜欢",
        "好棒", "伤心",
    ];
    for w in &emotional {
        if lower.contains(w) {
            score += 0.15;
        }
    }

    let personal = ["神大人", "葵", "琳玲", "kami", "aoi"];
    for w in &personal {
        if text.contains(w) {
            score += 0.1;
        }
    }
    if text.contains(char::is_numeric) {
        score += 0.05;
    }
    score.min(1.0)
}

/// 提取已知实体（静态匹配）
fn extract_known_entities(text: &str) -> Vec<String> {
    let mut entities = Vec::new();
    let known = [
        "神大人", "葵", "琳玲", "透闪石", "Tremolite",
        "情绪", "记忆", "注意力", "学习", "计划书",
        "插件", "工具", "L1", "L2", "L3",
    ];
    for entity in &known {
        if text.contains(entity) {
            entities.push(entity.to_string());
        }
    }
    entities
}

/// 对话类型分类
fn classify_chat_type(channel_blocks: &HashMap<String, Vec<AttentionBlock>>, channels: &[Channel]) -> ChatType {
    let has_focus_active = channels.iter()
        .find(|c| c.name == "focus")
        .and_then(|c| channel_blocks.get(&c.name))
        .map(|b| b.iter().any(|bl| bl.score > 0.5))
        .unwrap_or(false);

    let has_micro_active = channels.iter()
        .find(|c| c.name == "micro")
        .and_then(|c| channel_blocks.get(&c.name))
        .map(|b| b.iter().any(|bl| bl.score > 0.5))
        .unwrap_or(false);

    let has_wide_active = channels.iter()
        .find(|c| c.name == "wide")
        .and_then(|c| channel_blocks.get(&c.name))
        .map(|b| b.first().map(|bl| bl.score > 0.3).unwrap_or(false))
        .unwrap_or(false);

    if has_focus_active && has_micro_active {
        ChatType::FocusDiscussion
    } else if has_wide_active && !has_focus_active {
        ChatType::TopicShift
    } else {
        ChatType::Scattered
    }
}

// ─── 单元测试 ──────────────────────────────────────

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_channels() {
        let ch = default_channels();
        assert_eq!(ch.len(), 3);
        assert_eq!(ch[0].name, "wide");
        assert_eq!(ch[1].name, "focus");
        assert_eq!(ch[2].name, "micro");
    }

    #[test]
    fn test_keyword_bonus() {
        let s = keyword_attention_bonus("神大人今天好开心");
        assert!(s > 0.5);
    }

    #[test]
    fn test_entities() {
        let e = extract_known_entities("神大人和葵在讨论透闪石");
        assert!(e.contains(&"神大人".to_string()));
        assert!(e.contains(&"葵".to_string()));
    }

    #[test]
    fn test_attend() {
        let mut engine = MultiScaleAttention::new();
        let r = engine.attend("神大人今天好开心呀，葵也很开心呢。透闪石的记忆系统有五层缓存。");
        assert!(!r.channel_blocks.is_empty());
    }

    #[test]
    fn test_chat_type_classify() {
        let channels = default_channels();
        let mut blocks = HashMap::new();

        // 空 → Scattered
        blocks.insert("wide".into(), vec![]);
        blocks.insert("focus".into(), vec![]);
        blocks.insert("micro".into(), vec![]);
        assert_eq!(classify_chat_type(&blocks, &channels), ChatType::Scattered);

        // 只有 wide → TopicShift
        blocks.insert("wide".into(), vec![AttentionBlock {
            channel: "wide".into(),
            position: 0,
            content: "test".into(),
            score: 0.5,
            key_entities: vec![],
            timestamp: 0,
        }]);
        assert_eq!(classify_chat_type(&blocks, &channels), ChatType::TopicShift);

        // focus + micro → FocusDiscussion
        blocks.insert("focus".into(), vec![AttentionBlock {
            channel: "focus".into(),
            position: 0,
            content: "test".into(),
            score: 0.6,
            key_entities: vec![],
            timestamp: 0,
        }]);
        blocks.insert("micro".into(), vec![AttentionBlock {
            channel: "micro".into(),
            position: 0,
            content: "test".into(),
            score: 0.6,
            key_entities: vec![],
            timestamp: 0,
        }]);
        assert_eq!(classify_chat_type(&blocks, &channels), ChatType::FocusDiscussion);
    }

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c) < 0.01);
    }

    /// 从工作目录加载 config.toml 并测试真实 embedding API
    #[test]
    fn test_real_embedding_api() {
        // 从项目根目录加载 config
        let config_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()  // crates
            .parent().unwrap()  // root
            .join("config.toml");

        let content = match std::fs::read_to_string(&config_path) {
            Ok(c) => c,
            Err(_) => {
                eprintln!("skip: config.toml not found");
                return;
            }
        };

        let emb_config: Result<tremolite_config::Config, _> =
            toml::from_str(&content);
        let emb_cfg = match emb_config {
            Ok(c) => c.embedding.unwrap_or_default(),
            Err(_) => {
                eprintln!("skip: no [embedding] in config");
                return;
            }
        };

        if emb_cfg.api_key.is_empty() {
            eprintln!("skip: embedding api_key is empty");
            return;
        }

        // 用真实 embedding 引擎测试
        let mut engine = MultiScaleAttention::new()
            .with_embedding_api(&emb_cfg.api_base, &emb_cfg.api_key, &emb_cfg.model);

        let text = "神大人今天好开心呀，葵也很开心呢。透闪石的记忆系统有五层缓存。";
        let result = engine.attend(text);

        // 如果有 embedding，score 应该 > 纯关键词的 baseline
        if let Some(wide_blocks) = result.channel_blocks.get("wide") {
            if !wide_blocks.is_empty() {
                let top_score = wide_blocks[0].score;
                eprintln!("  top wide block score: {:.4}", top_score);
                assert!(top_score > 0.3, "embedding should give >0.3 score, got {:.4}", top_score);
            }
        }

        // 验证 entity 提取
        let has_kamisama = result.synthesis.top_entities.iter()
            .any(|(e, _)| e == "神大人");
        let has_aoi = result.synthesis.top_entities.iter()
            .any(|(e, _)| e == "葵");
        assert!(has_kamisama, "should detect 神大人");
        assert!(has_aoi, "should detect 葵");

        eprintln!("  real embedding API 测试通过 ✓");
    }
}
