use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ─── 嵌入服务配置 ────────────────────────────────

/// 嵌入服务配置——从 config.toml 读取
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Provider 类型：siliconflow / openai / cli
    pub provider: String,
    /// API 地址（siliconflow / openai）
    pub api_url: String,
    /// API key
    pub api_key: String,
    /// 模型名
    pub model: String,
    /// 输出维度
    pub dimensions: usize,
    /// 缓存大小（嵌入结果 LRU 上限）
    pub cache_size: usize,
    /// 超时秒数
    pub timeout_secs: u64,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: "siliconflow".into(),
            api_url: "https://api.siliconflow.cn/v1/embeddings".into(),
            api_key: String::new(),
            model: "BAAI/bge-large-zh-v1.5".into(),
            dimensions: 1024,
            cache_size: 500,
            timeout_secs: 10,
        }
    }
}

impl EmbeddingConfig {
    // ── Builder 链式方法 ──

    pub fn with_api_key(mut self, key: &str) -> Self {
        self.api_key = key.to_string();
        self
    }

    pub fn with_api_url(mut self, url: &str) -> Self {
        self.api_url = url.to_string();
        self
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    pub fn with_provider(mut self, provider: &str) -> Self {
        self.provider = provider.to_string();
        self
    }

    pub fn with_dimensions(mut self, dims: usize) -> Self {
        self.dimensions = dims;
        self
    }

    pub fn with_cache_size(mut self, size: usize) -> Self {
        self.cache_size = size;
        self
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    // ── 运行时 setter ──

    /// 修改 API key（不会重建连接，下次请求生效）
    pub fn set_api_key(&mut self, key: &str) {
        self.api_key = key.to_string();
    }

    /// 切换模型（尺寸不变时不影响运行）
    pub fn set_model(&mut self, model: &str) {
        self.model = model.to_string();
    }

    /// 切换 API 地址
    pub fn set_api_url(&mut self, url: &str) {
        self.api_url = url.to_string();
    }

    /// 切换 provider 类型
    pub fn set_provider(&mut self, provider: &str) {
        self.provider = provider.to_string();
    }

    /// 覆盖整个配置
    pub fn apply(&mut self, other: &EmbeddingConfig) {
        self.provider = other.provider.clone();
        self.api_url = other.api_url.clone();
        self.api_key = other.api_key.clone();
        self.model = other.model.clone();
        self.dimensions = other.dimensions;
        self.cache_size = other.cache_size;
        self.timeout_secs = other.timeout_secs;
    }
}

// ─── EmbeddingService trait ──────────────────────

/// 计算两个向量的余弦相似度——独立函数，不依赖 trait 实现
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f64 = a.iter().zip(b.iter()).map(|(x, y)| (*x as f64) * (*y as f64)).sum();
    let norm_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let norm_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).clamp(0.0, 1.0)
}

/// 嵌入服务——将文本转为向量
/// 每个实现可以对接不同的 embedding 来源
pub trait EmbeddingService: Send + Sync {
    /// 将单段文本转为向量
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError>;

    /// 批量转换（某些 API 支持 batch 以节省调用次数）
    fn batch_embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        texts.iter().map(|t| self.embed(t)).collect()
    }

    /// 返回配置引用
    fn config(&self) -> &EmbeddingConfig;
}

// ─── 错误类型 ──────────────────────────────────────

#[derive(Debug, Clone)]
pub enum EmbeddingError {
    ApiError(String),
    ParseError(String),
    ConfigError(String),
    NetworkError(String),
}

impl std::fmt::Display for EmbeddingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EmbeddingError::ApiError(msg) => write!(f, "API error: {}", msg),
            EmbeddingError::ParseError(msg) => write!(f, "parse error: {}", msg),
            EmbeddingError::ConfigError(msg) => write!(f, "config error: {}", msg),
            EmbeddingError::NetworkError(msg) => write!(f, "network error: {}", msg),
        }
    }
}

// ─── 硅基流动实现 ────────────────────────────────

/// 通过硅基流动 API 调用 BGE 模型
pub struct SiliconFlowEmbedder {
    config: EmbeddingConfig,
    cache: Mutex<EmbeddingCache>,
    client: ureq::Agent,
}

impl SiliconFlowEmbedder {
    pub fn new(config: EmbeddingConfig) -> Self {
        let cache = EmbeddingCache::new(config.cache_size);
        let client = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(config.timeout_secs))
            .timeout_read(Duration::from_secs(config.timeout_secs * 2))
            .build();
        Self {
            config,
            cache: Mutex::new(cache),
            client,
        }
    }

    /// 运行时更新配置——换了 API key 或模型后调用，不影响正在跑的请求
    pub fn reconfigure(&mut self, config: EmbeddingConfig) {
        self.config = config;
        // 清空缓存，避免用旧 API key 的结果蒙混
        if let Ok(mut cache) = self.cache.lock() {
            cache.clear();
        }
    }

    /// 获取当前配置的引用
    pub fn current_config(&self) -> &EmbeddingConfig {
        &self.config
    }
}

impl EmbeddingService for SiliconFlowEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        // 查缓存
        if let Ok(cache) = self.cache.lock() {
            if let Some(cached) = cache.get(text) {
                return Ok(cached);
            }
        }

        // 构建请求体
        let body = serde_json::json!({
            "model": self.config.model,
            "input": text,
            "encoding_format": "float"
        });

        // 发送请求
        let response = self
            .client
            .post(&self.config.api_url)
            .set("Authorization", &format!("Bearer {}", self.config.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| EmbeddingError::NetworkError(e.to_string()))?;

        let resp_text = response
            .into_string()
            .map_err(|e| EmbeddingError::NetworkError(e.to_string()))?;

        let parsed: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| EmbeddingError::ParseError(e.to_string()))?;

        // 提取 embedding 向量
        let embedding = parsed["data"][0]["embedding"]
            .as_array()
            .ok_or_else(|| EmbeddingError::ParseError("no embedding in response".into()))?;

        let vec: Vec<f32> = embedding
            .iter()
            .filter_map(|v| v.as_f64().map(|f| f as f32))
            .collect();

        if vec.len() != self.config.dimensions {
            return Err(EmbeddingError::ParseError(format!(
                "dimension mismatch: expected {}, got {}",
                self.config.dimensions,
                vec.len()
            )));
        }

        // 写入缓存
        if let Ok(mut cache) = self.cache.lock() {
            cache.set(text, &vec);
        }

        Ok(vec)
    }

    fn batch_embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbeddingError> {
        // 硅基流动支持 batch，一次请求所有文本
        let body = serde_json::json!({
            "model": self.config.model,
            "input": texts,
            "encoding_format": "float"
        });

        let response = self
            .client
            .post(&self.config.api_url)
            .set("Authorization", &format!("Bearer {}", self.config.api_key))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| EmbeddingError::NetworkError(e.to_string()))?;

        let resp_text = response
            .into_string()
            .map_err(|e| EmbeddingError::NetworkError(e.to_string()))?;

        let parsed: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| EmbeddingError::ParseError(e.to_string()))?;

        let data = parsed["data"]
            .as_array()
            .ok_or_else(|| EmbeddingError::ParseError("no data array in response".into()))?;

        let mut results = Vec::new();
        for item in data {
            let embedding = item["embedding"]
                .as_array()
                .ok_or_else(|| EmbeddingError::ParseError("no embedding in item".into()))?;
            let vec: Vec<f32> = embedding
                .iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect();
            results.push(vec);
        }

        Ok(results)
    }

    fn config(&self) -> &EmbeddingConfig {
        &self.config
    }
}

// ─── 嵌入结果 LRU 缓存 ──────────────────────────

/// 轻量 LRU 缓存——缓存已计算的嵌入结果，避免重复 API 调用
pub struct EmbeddingCache {
    max_size: usize,
    entries: Vec<(String, Vec<f32>, u64)>, // (text, embedding, last_access)
}

impl EmbeddingCache {
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            entries: Vec::with_capacity(max_size),
        }
    }

    pub fn get(&self, text: &str) -> Option<Vec<f32>> {
        // 注意：这里不更新时间戳，因为是不可变引用
        // 热度由外面管理，这里只做查找
        self.entries
            .iter()
            .find(|(t, _, _)| t == text)
            .map(|(_, emb, _)| emb.clone())
    }

    pub fn set(&mut self, text: &str, embedding: &[f32]) {
        // 如果已存在，更新
        if let Some(pos) = self.entries.iter().position(|(t, _, _)| t == text) {
            self.entries[pos].1 = embedding.to_vec();
            self.entries[pos].2 = now_millis();
            return;
        }

        // 超过上限，移除最早访问的
        if self.entries.len() >= self.max_size {
            if let Some(pos) = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.2)
                .map(|(i, _)| i)
            {
                self.entries.remove(pos);
            }
        }

        self.entries.push((text.to_string(), embedding.to_vec(), now_millis()));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(0))
        .as_millis() as u64
}

// ─── 单元测试 ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = super::cosine_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = super::cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_partial() {
        let a = vec![1.0, 1.0];
        let b = vec![1.0, 0.0];
        let sim = super::cosine_similarity(&a, &b);
        // cos(45°) ≈ 0.707
        assert!((sim - 0.707).abs() < 0.01);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        let sim = super::cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_basic() {
        let mut cache = EmbeddingCache::new(10);
        let emb = vec![0.1, 0.2, 0.3];
        cache.set("hello", &emb);
        let cached = cache.get("hello");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap(), emb);
    }

    #[test]
    fn test_cache_lru_eviction() {
        let mut cache = EmbeddingCache::new(3);
        cache.set("a", &[1.0]);
        cache.set("b", &[2.0]);
        cache.set("c", &[3.0]);
        cache.set("d", &[4.0]); // 应淘汰 "a"
        assert!(cache.get("a").is_none());
        assert!(cache.get("b").is_some());
        assert!(cache.get("d").is_some());
    }

    #[test]
    fn test_cache_update() {
        let mut cache = EmbeddingCache::new(5);
        cache.set("key", &[1.0, 2.0]);
        cache.set("key", &[3.0, 4.0]);
        let cached = cache.get("key").unwrap();
        assert_eq!(cached, vec![3.0, 4.0]);
    }

    #[test]
    fn test_embedding_config_default() {
        let config = EmbeddingConfig::default();
        assert_eq!(config.provider, "siliconflow");
        assert_eq!(config.dimensions, 1024);
        assert_eq!(config.model, "BAAI/bge-large-zh-v1.5");
    }
}
