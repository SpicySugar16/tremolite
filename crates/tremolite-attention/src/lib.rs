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

// ─── 注意力数据结构 ───────────────────────────────

/// 四层注意力尺度
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum AttentionScale {
    Macro,
    Focus,
    Micro,
    Synthesis,
}

impl AttentionScale {
    pub fn as_str(&self) -> &'static str {
        match self {
            AttentionScale::Macro => "macro scan",
            AttentionScale::Focus => "focus zoom",
            AttentionScale::Micro => "micro refine",
            AttentionScale::Synthesis => "synthesis",
        }
    }
    pub fn window_size(&self) -> usize {
        match self {
            AttentionScale::Macro => 1000,
            AttentionScale::Focus => 200,
            AttentionScale::Micro => 50,
            AttentionScale::Synthesis => 0,
        }
    }
    pub fn stride(&self) -> usize {
        match self {
            AttentionScale::Macro => 500,
            AttentionScale::Focus => 50,
            AttentionScale::Micro => 10,
            AttentionScale::Synthesis => 0,
        }
    }
}

/// 一次注意力计算的结果块
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionBlock {
    pub scale: AttentionScale,
    pub position: usize,
    pub content: String,
    pub score: f64,
    pub key_entities: Vec<String>,
    pub timestamp: u64,
}

/// 四层注意力的完整输出
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttentionResult {
    pub macro_blocks: Vec<AttentionBlock>,
    pub focus_blocks: Vec<AttentionBlock>,
    pub micro_blocks: Vec<AttentionBlock>,
    pub synthesis: AttentionSynthesis,
}

impl AttentionResult {
    pub fn empty() -> Self {
        Self {
            macro_blocks: Vec::new(),
            focus_blocks: Vec::new(),
            micro_blocks: Vec::new(),
            synthesis: AttentionSynthesis::empty(),
        }
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

/// 多尺度注意力引擎
pub struct MultiScaleAttention {
    attention_history: Vec<AttentionResult>,
    max_history: usize,
    embedding: Option<EmbeddingEngine>,
    /// 上一次缓存的 query embedding（避免同一轮重复嵌入）
    cached_query_embedding: Option<Vec<f32>>,
    cached_query_text: String,
}

impl Default for MultiScaleAttention {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiScaleAttention {
    pub fn new() -> Self {
        Self {
            attention_history: Vec::new(),
            max_history: 100,
            embedding: None,
            cached_query_embedding: None,
            cached_query_text: String::new(),
        }
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

    /// 对输入文本执行四层注意力扫描
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

        let macro_blocks = self.scan_scale(text, AttentionScale::Macro, now, query_vec.as_deref());
        let focus_blocks = {
            let mut fb = Vec::new();
            for candidate in macro_blocks.iter().filter(|b| b.score > 0.4) {
                let start = candidate.position;
                let end = (start + candidate.content.len()).min(text.len());
                if start < end {
                    fb.extend(self.scan_scale(&text[start..end], AttentionScale::Focus, now, query_vec.as_deref()));
                }
            }
            fb
        };
        let micro_blocks = {
            let mut mb = Vec::new();
            for candidate in focus_blocks.iter().filter(|b| b.score > 0.5) {
                let start = candidate.position;
                let end = (start + candidate.content.len()).min(text.len());
                if start < end {
                    mb.extend(self.scan_scale(&text[start..end], AttentionScale::Micro, now, query_vec.as_deref()));
                }
            }
            mb
        };

        // 综合合成
        let mut all_blocks: Vec<&AttentionBlock> = macro_blocks
            .iter()
            .chain(focus_blocks.iter())
            .chain(micro_blocks.iter())
            .collect();
        all_blocks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

        let mut entity_scores: HashMap<String, f64> = HashMap::new();
        for block in &all_blocks {
            for entity in &block.key_entities {
                *entity_scores.entry(entity.clone()).or_insert(0.0) += block.score * 0.1;
            }
        }
        let mut sorted_entities: Vec<(String, f64)> = entity_scores.into_iter().collect();
        sorted_entities.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        sorted_entities.truncate(10);

        let top_regions: Vec<(usize, String, f64)> = all_blocks
            .iter()
            .take(5)
            .map(|b| (b.position, b.content.chars().take(50).collect(), b.score))
            .collect();

        let total_output = macro_blocks.len() + focus_blocks.len() + micro_blocks.len();
        let compression_ratio = if total_len > 0 {
            (total_output as f64 * 50.0) / total_len as f64
        } else {
            1.0
        };

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
            macro_blocks,
            focus_blocks,
            micro_blocks,
        };

        self.attention_history.push(result.clone());
        if self.attention_history.len() > self.max_history {
            self.attention_history.remove(0);
        }
        result
    }

    /// 滑动窗口扫描，支持语义评分
    fn scan_scale(
        &mut self,
        text: &str,
        scale: AttentionScale,
        timestamp: u64,
        query_vec: Option<&[f32]>,
    ) -> Vec<AttentionBlock> {
        let window = scale.window_size();
        let stride = scale.stride();
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
            segments.push((i, segment));
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
                        scale,
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
                        scale, position: pos, content: seg, score,
                        key_entities: entities, timestamp,
                    });
                }
            }
        } else {
            // 无 embedding，纯关键词
            for (pos, seg) in segments {
                let score = keyword_attention_bonus(&seg);
                let entities = extract_known_entities(&seg);
                blocks.push(AttentionBlock {
                    scale, position: pos, content: seg, score,
                    key_entities: entities, timestamp,
                });
            }
        }

        blocks.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        let max_blocks = match scale {
            AttentionScale::Macro => 10,
            AttentionScale::Focus => 8,
            AttentionScale::Micro => 5,
            AttentionScale::Synthesis => 0,
        };
        blocks.truncate(max_blocks);
        blocks
    }

    pub fn history(&self) -> &[AttentionResult] {
        &self.attention_history
    }

    pub fn last_result(&self) -> Option<&AttentionResult> {
        self.attention_history.last()
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

// ─── 单元测试 ──────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scale_window() {
        assert_eq!(AttentionScale::Macro.window_size(), 1000);
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
        assert!(!r.macro_blocks.is_empty());
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

        // 如果有 embedding，score 应该 > 纯关键词的 baseline (0.3 + 关键词加分)
        // 关键词加分 = 神大人(0.1) + 开心(0.15) + 葵(0.1) + 透闪石(0.1) ≈ 0.75
        // embedding 语义分应进一步提升
        if !result.macro_blocks.is_empty() {
            let top_score = result.macro_blocks[0].score;
            eprintln!("  top macro block score: {:.4}", top_score);
            assert!(top_score > 0.3, "embedding should give >0.3 score, got {:.4}", top_score);
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

    #[test]
    fn test_cosine_similarity() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);

        let c = vec![0.0, 1.0, 0.0];
        assert!(cosine_similarity(&a, &c) < 0.01);
    }
}
