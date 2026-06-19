use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::env;

/// 用户级运行时目录（用于隔离敏感配置）
const RUNTIME_DIR: &str = "~/.tremolite";

use tremolite_llm::{
    ProviderRegistry, LLMProvider,
    OpenAIProvider, DeepSeekProvider, OllamaProvider,
};

// ─── 错误类型 ─────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Environment variable '{0}' is not set")]
    MissingEnv(String),
}

// ─── 顶层配置 ─────────────────────────────────────

/// 透闪石完整配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub core: CoreConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub embedding: Option<EmbeddingConfig>,
    /// 消息通道配置
    #[serde(default)]
    pub channels: HashMap<String, ChannelConfig>,
    /// 定时任务配置
    #[serde(default)]
    pub cron: CronConfig,
    /// MCP 服务器配置
    #[serde(default)]
    pub mcp: McpConfig,
}

/// 核心配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreConfig {
    /// 数据目录（记忆、技能、计划书的存放位置）
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
    /// 灵魂设定文件路径（默认读取 SOUL.md）
    #[serde(default = "default_soul_path")]
    pub soul_path: String,
    /// 系统提示词（可选。如不设置则从 SOUL.md 加载）
    #[serde(default)]
    pub system_prompt: Option<String>,
}

fn default_data_dir() -> String {
    "./data/tremolite".into()
}

fn default_soul_path() -> String {
    "./SOUL.md".into()
}

/// 从文件加载灵魂设定。先读 config 中指定的 system_prompt，
/// 若未指定则读取 SOUL.md，若都不存在则返回空提示词让调用方自行处理。
fn load_soul_from_file(soul_path: &str, config_prompt: &Option<String>) -> String {
    if let Some(prompt) = config_prompt {
        if !prompt.is_empty() {
            return prompt.clone();
        }
    }
    let path = PathBuf::from(soul_path);
    if path.exists() {
        std::fs::read_to_string(&path).unwrap_or_default()
    } else {
        String::new()
    }
}

impl CoreConfig {
    pub fn soul(&self) -> String {
        load_soul_from_file(&self.soul_path, &self.system_prompt)
    }
}

/// Embedding 配置（硅基流动 OpenAI 兼容 API）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// API 基地址
    pub api_base: String,
    /// API Key（支持 ${ENV_VAR} 引用）
    pub api_key: String,
    /// 模型名
    pub model: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            api_base: "https://api.siliconflow.cn/v1".into(),
            api_key: "".into(),
            model: "BAAI/bge-m3".into(),
        }
    }
}

// ─── 定时任务配置 ──────────────────────────────────

/// 定时任务配置（顶层）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronConfig {
    /// cron job 定义列表
    #[serde(default)]
    pub jobs: HashMap<String, CronJobConfig>,
    /// 是否默认启用调度器
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

/// 单个 cron job 的配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJobConfig {
    /// job 名称（可选，默认用 config key）
    #[serde(default)]
    pub name: Option<String>,
    /// 调度计划
    pub schedule: CronScheduleConfig,
    /// 执行动作
    pub action: CronActionConfig,
    /// 是否默认启用
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// 调度计划配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CronScheduleConfig {
    #[serde(rename = "every")]
    EverySecs(u64),
    #[serde(rename = "daily")]
    Daily { hour: u8, minute: u8 },
    #[serde(rename = "once")]
    Once { delay_secs: u64 },
    #[serde(rename = "cron")]
    CronExpr(String),
}

/// 执行动作配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CronActionConfig {
    #[serde(rename = "shell")]
    Shell { command: String },
    #[serde(rename = "prompt")]
    LlmPrompt { prompt: String },
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            jobs: HashMap::new(),
            enabled: true,
        }
    }
}

/// LLM 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    /// 默认使用的 provider 名称
    #[serde(default)]
    pub default: Option<String>,
    /// 所有 provider 的配置
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
}

impl Default for CoreConfig {
    fn default() -> Self {
        Self {
            data_dir: default_data_dir(),
            soul_path: default_soul_path(),
            system_prompt: None,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            default: None,
            providers: HashMap::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            core: CoreConfig::default(),
            llm: LlmConfig::default(),
            embedding: None,
            channels: HashMap::new(),
            cron: CronConfig::default(),
            mcp: McpConfig::default(),
        }
    }
}

// ─── Provider 配置 ────────────────────────────────

/// 单个 LLM Provider 的配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ProviderConfig {
    /// OpenAI 兼容 API（GPT、DeepSeek、OpenRouter 等）
    #[serde(rename = "openai")]
    OpenAI {
        api_key: String,
        model: String,
        #[serde(default = "default_openai_url")]
        base_url: String,
        /// 超时时间（秒），默认 120
        #[serde(default = "default_timeout")]
        timeout_secs: u64,
    },
    /// DeepSeek（可被 openai 替代，保留为独立选项）
    #[serde(rename = "deepseek")]
    DeepSeek {
        api_key: String,
        #[serde(default = "default_deepseek_model")]
        model: String,
        /// 超时时间（秒），默认 180
        #[serde(default = "default_deepseek_timeout")]
        timeout_secs: u64,
    },
    /// Ollama 本地模型
    #[serde(rename = "ollama")]
    Ollama {
        model: String,
        #[serde(default = "default_ollama_url")]
        base_url: String,
        /// 超时时间（秒），默认 300
        #[serde(default = "default_ollama_timeout")]
        timeout_secs: u64,
    },
}

// ─── 通道配置 ──────────────────────────────────────

/// 消息通道配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ChannelConfig {
    /// HTTP 回调通道（webhook 接收器）
    #[serde(rename = "http")]
    Http {
        /// 监听地址
        listen: String,
        /// 通道名称（可选，默认使用 config key）
        #[serde(default)]
        name: Option<String>,
    },
    /// NapCat WebSocket 通道（未来扩展）
    #[serde(rename = "napcat")]
    NapCat {
        /// WebSocket 服务器 URL
        ws_url: String,
        /// 通道名称（可选）
        #[serde(default)]
        name: Option<String>,
    },
    /// QQ 开放平台 Bot 官方通道
    #[serde(rename = "qqbot")]
    QqBot {
        /// QQ 开放平台 app_id
        app_id: String,
        /// QQ 开放平台 client_secret（用于 OAuth2 获取 access_token）
        client_secret: String,
        /// 原始 token（可选，部分场景备用）
        #[serde(default)]
        token: Option<String>,
        /// 是否为正式环境（默认 false=沙箱）
        #[serde(default)]
        production: bool,
        /// 通道名称（可选）
        #[serde(default)]
        name: Option<String>,
    },
}

impl ChannelConfig {
    /// 替换配置值中的 ${VAR} 为环境变量
    fn resolve_env(&self) -> Result<Self, ConfigError> {
        Ok(match self {
            ChannelConfig::Http { listen, name } => ChannelConfig::Http {
                listen: resolve_env_string(listen)?,
                name: name.clone(),
            },
            ChannelConfig::NapCat { ws_url, name } => ChannelConfig::NapCat {
                ws_url: resolve_env_string(ws_url)?,
                name: name.clone(),
            },
            ChannelConfig::QqBot { app_id, client_secret, token, production, name } => {
                ChannelConfig::QqBot {
                    app_id: resolve_env_string(app_id)?,
                    client_secret: resolve_env_string(client_secret)?,
                    token: token.as_ref().map(|t| resolve_env_string(t)).transpose()?,
                    production: *production,
                    name: name.clone(),
                }
            }
        })
    }
}

/// MCP 服务器配置列表
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// MCP 服务器列表
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// 单个 MCP 服务器的配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// 服务名称
    pub name: String,
    /// MCP 服务器 URL
    pub url: String,
    /// 工具名称前缀（可选）
    #[serde(default)]
    pub prefix: String,
    /// 超时秒数
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

pub use McpServerConfig as McpServer;

fn default_timeout() -> u64 { 120 }
fn default_deepseek_timeout() -> u64 { 180 }
fn default_ollama_timeout() -> u64 { 300 }

fn default_openai_url() -> String {
    "https://api.openai.com/v1".into()
}

fn default_deepseek_model() -> String {
    "deepseek-chat".into()
}

fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}

// ─── Config 加载 ──────────────────────────────────

impl Config {
    /// 从文件加载配置
    /// 查找顺序：
    ///   1. 指定的 path（如有）
    ///   2. `./config.toml`（仓库本地配置——建议只放示例/模板）
    ///   3. `~/.tremolite/config.toml`（用户级运行时配置——存放敏感信息）
    pub fn load(path: Option<&str>) -> Result<Self, ConfigError> {
        // 加载环境变量：先本地 .env，再用户级 .env（后者覆盖前者）
        load_env_file("./.env");
        let runtime_env = expand_tilde("~/.tremolite/.env");
        load_env_file(&runtime_env);

        // 决定用哪个配置路径
        let config_path = resolve_config_path(path);
        let content = std::fs::read_to_string(&config_path)?;
        let mut config: Config = toml::from_str(&content)?;

        // 解析所有 env 变量引用
        config.resolve_env_vars()?;

        Ok(config)
    }

    /// 从字符串加载（用于测试）
    pub fn from_str(content: &str) -> Result<Self, ConfigError> {
        let mut config: Config = toml::from_str(content)?;
        config.resolve_env_vars()?;
        Ok(config)
    }

    /// 替换所有配置值中的 ${VAR_NAME} 为环境变量
    fn resolve_env_vars(&mut self) -> Result<(), ConfigError> {
        let providers = std::mem::take(&mut self.llm.providers);

        let resolved: HashMap<String, ProviderConfig> = providers
            .into_iter()
            .map(|(name, provider)| {
                let resolved = provider.resolve_env()?;
                Ok((name, resolved))
            })
            .collect::<Result<HashMap<_, _>, ConfigError>>()?;

        self.llm.providers = resolved;

        // 同时解析 embedding 配置中的环境变量引用
        if let Some(ref mut emb) = self.embedding {
            emb.api_key = resolve_env_string(&emb.api_key)?;
            emb.api_base = resolve_env_string(&emb.api_base)?;
            emb.model = resolve_env_string(&emb.model)?;
        }

        // 解析通道配置中的环境变量引用
        let channels = std::mem::take(&mut self.channels);
        let resolved_channels: HashMap<String, ChannelConfig> = channels
            .into_iter()
            .map(|(name, channel)| {
                let resolved = channel.resolve_env()?;
                Ok((name, resolved))
            })
            .collect::<Result<HashMap<_, _>, ConfigError>>()?;
        self.channels = resolved_channels;

        Ok(())
    }

    /// 将配置的 provider 注册到 ProviderRegistry
    /// 返回可初始化的 registry，以及默认 provider 的名称
    pub fn initialize_providers(&self) -> Result<ProviderRegistry, ConfigError> {
        let mut registry = ProviderRegistry::new();

        for (name, provider) in &self.llm.providers {
            let boxed: Box<dyn LLMProvider> = match provider {
                ProviderConfig::OpenAI { api_key, model, base_url, timeout_secs } => {
                    Box::new(
                        OpenAIProvider::new(api_key, model)
                            .with_base_url(base_url)
                            .with_timeout(*timeout_secs)
                    )
                }
                ProviderConfig::DeepSeek { api_key, model, timeout_secs } => {
                    Box::new(
                        DeepSeekProvider::new(api_key, model)
                            .with_timeout(*timeout_secs)
                    )
                }
                ProviderConfig::Ollama { model, base_url, timeout_secs } => {
                    Box::new(
                        OllamaProvider::new(base_url, model)
                            .with_timeout(*timeout_secs)
                    )
                }
            };
            registry.register(name, boxed);
        }

        // 设置默认 provider
        if let Some(default_name) = &self.llm.default {
            registry.set_default(default_name)
                .map_err(|e| match e {
                    tremolite_llm::LlmError::ProviderNotFound(n) =>
                        ConfigError::Io(std::io::Error::new(
                            std::io::ErrorKind::NotFound,
                            format!("Default provider '{}' not found in config", n),
                        )),
                    _ => ConfigError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )),
                })?;
        }

        Ok(registry)
    }
}

// ─── Provider 配置的 env 变量解析 ─────────────────

impl ProviderConfig {
    /// 替换字段中的 ${VAR} 为实际环境变量值
    fn resolve_env(&self) -> Result<Self, ConfigError> {
        Ok(match self {
            ProviderConfig::OpenAI { api_key, model, base_url, timeout_secs } => {
                ProviderConfig::OpenAI {
                    api_key: resolve_env_string(api_key)?,
                    model: resolve_env_string(model)?,
                    base_url: resolve_env_string(base_url)?,
                    timeout_secs: *timeout_secs,
                }
            }
            ProviderConfig::DeepSeek { api_key, model, timeout_secs } => {
                ProviderConfig::DeepSeek {
                    api_key: resolve_env_string(api_key)?,
                    model: resolve_env_string(model)?,
                    timeout_secs: *timeout_secs,
                }
            }
            ProviderConfig::Ollama { model, base_url, timeout_secs } => {
                ProviderConfig::Ollama {
                    model: resolve_env_string(model)?,
                    base_url: resolve_env_string(base_url)?,
                    timeout_secs: *timeout_secs,
                }
            }
        })
    }
}

/// 将 `~` 替换为 `$HOME`
fn expand_tilde(s: &str) -> String {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Ok(home) = env::var("HOME") {
            return format!("{}/{}", home.trim_end_matches('/'), rest);
        }
    }
    s.to_string()
}

/// 决定配置文件的查找路径：
///   1. 指定的 path（如有）
///   2. `./config.toml`
///   3. `~/.tremolite/config.toml`
fn resolve_config_path(path: Option<&str>) -> PathBuf {
    if let Some(p) = path {
        return PathBuf::from(p);
    }
    // 优先本地配置（仓库中建议只放 config.example.toml，config.toml 已 .gitignore）
    if Path::new("./config.toml").exists() {
        return PathBuf::from("./config.toml");
    }
    // 回退到用户级运行时配置
    expand_tilde("~/.tremolite/config.toml").into()
}

/// 尝试从 .env 文件加载环境变量
/// 格式：KEY=VALUE，忽略空行和 # 注释
fn load_env_file(path: &str) {
    let p = Path::new(path);
    if !p.exists() {
        return;
    }
    if let Ok(content) = std::fs::read_to_string(p) {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some(eq_pos) = trimmed.find('=') {
                let key = trimmed[..eq_pos].trim();
                let value = trimmed[eq_pos + 1..].trim();
                env::set_var(key, value);
            }
        }
    }
}

/// 替换字符串中的 ${VAR_NAME} 为环境变量
/// 如果环境变量不存在则返回错误
fn resolve_env_string(s: &str) -> Result<String, ConfigError> {
    let mut result = s.to_string();

    // 匹配 ${...} 模式
    while let Some(start) = result.find("${") {
        if let Some(end) = result[start..].find('}') {
            let var_name = result[start + 2..start + end].to_string();
            let var_value = env::var(&var_name)
                .map_err(|_| ConfigError::MissingEnv(var_name.clone()))?;
            result.replace_range(start..start + end + 1, &var_value);
        } else {
            break;
        }
    }

    Ok(result)
}

// ─── 单元测试 ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.core.data_dir, "./data/tremolite");
        assert!(config.llm.providers.is_empty());
    }

    #[test]
    fn test_parse_full_config() {
        let toml_str = r#"
[core]
data_dir = "./data/tremolite"
system_prompt = "你是透闪石，一个自主AI agent"

[llm]
default = "deepseek"

[llm.providers.deepseek]
type = "deepseek"
api_key = "sk-test-key"
model = "deepseek-chat"

[llm.providers.ollama]
type = "ollama"
model = "qwen2.5:7b"
base_url = "http://localhost:11434"
"#;
        let config = Config::from_str(toml_str).unwrap();
        assert_eq!(config.core.data_dir, "./data/tremolite");
        assert_eq!(config.llm.default.as_deref(), Some("deepseek"));
        assert_eq!(config.llm.providers.len(), 2);
    }

    #[test]
    fn test_env_var_resolution() {
        // 先用一个没有 ${} 的值测试解析不报错
        let result = resolve_env_string("sk-test-key");
        assert_eq!(result.unwrap(), "sk-test-key");
    }

    #[test]
    fn test_provider_initialization() {
        let config = Config {
            core: CoreConfig::default(),
            llm: LlmConfig {
                default: Some("ollama".into()),
                providers: {
                    let mut m = HashMap::new();
                    m.insert("ollama".into(), ProviderConfig::Ollama {
                        model: "qwen2.5:7b".into(),
                        base_url: "http://localhost:11434".into(),
                        timeout_secs: 300,
                    });
                    m
                },
            },
            embedding: None,
            channels: Default::default(),
            cron: CronConfig::default(),
            mcp: McpConfig::default(),
        };

        let registry = config.initialize_providers().unwrap();
        assert!(registry.get_default().is_some());
        assert_eq!(registry.list().len(), 1);
    }
}
