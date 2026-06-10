use crate::{LLMProvider, Message, ToolDefinition, LlmResponse, LlmError, StreamChunk, StreamIterator};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Ollama 本地模型 provider
pub struct OllamaProvider {
    base_url: String,
    model: String,
    client: reqwest::blocking::Client,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    num_predict: i32,
    temperature: f64,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
    done: bool,
}

#[derive(Deserialize)]
struct ResponseMessage {
    role: String,
    content: String,
}

impl OllamaProvider {
    pub fn new(base_url: &str, model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// 默认本地 Ollama（http://localhost:11434）
    pub fn local(model: &str) -> Self {
        Self::new("http://localhost:11434", model)
    }

    /// 设置超时时间（秒）
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(secs))
            .build()
            .expect("Failed to create HTTP client");
        self
    }
}

impl LLMProvider for OllamaProvider {
    fn name(&self) -> &str {
        "ollama"
    }

    fn chat(&self, messages: &[Message], _tools: &[ToolDefinition]) -> Result<LlmResponse, LlmError> {
        // Ollama 的 chat API 不直接支持 tool calling，所以忽略 tools
        let request_body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            stream: false,
            options: OllamaOptions {
                num_predict: 2048,
                temperature: 0.7,
            },
        };

        let response = self.client
            .post(format!("{}/api/chat", self.base_url))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
            .map_err(|e| LlmError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(LlmError::Api(format!("HTTP {}: {}", status, body)));
        }

        let chat_resp: ChatResponse = response
            .json()
            .map_err(|e| LlmError::Parse(e.to_string()))?;

        Ok(LlmResponse {
            content: chat_resp.message.content,
            tool_calls: Vec::new(),
            finish_reason: if chat_resp.done { "stop".into() } else { "unknown".into() },
            usage: None,
        })
    }

    fn chat_stream(&self, _messages: &[Message], _tools: &[ToolDefinition]) -> Result<Box<dyn StreamIterator>, LlmError> {
        Err(LlmError::Api("Streaming not implemented yet".into()))
    }

    fn models(&self) -> Vec<String> {
        // 可以调用 /api/tags 获取，但先简单返回
        vec![self.model.clone()]
    }
}
