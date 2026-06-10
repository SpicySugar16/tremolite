use crate::{LLMProvider, Message, ToolDefinition, LlmResponse, LlmError, StreamChunk, StreamIterator};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::time::Duration;

pub struct DeepSeekProvider {
    api_key: String,
    model: String,
    client: reqwest::blocking::Client,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ToolDefinition>,
    stream: bool,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
    finish_reason: String,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: Option<String>,
    tool_calls: Option<Vec<ResponseToolCall>>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: ResponseFunction,
}

#[derive(Deserialize)]
struct ResponseFunction {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

/// DeepSeek SSE 流式 delta
#[derive(Deserialize)]
struct StreamDelta {
    choices: Vec<StreamChoice>,
}

#[derive(Deserialize)]
struct StreamChoice {
    delta: StreamDeltaContent,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct StreamDeltaContent {
    #[serde(default)]
    content: Option<String>,
}

impl DeepSeekProvider {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(180))
                .no_proxy()
                .build()
                .expect("Failed to create HTTP client"),
        }
    }

    /// 设置超时时间（秒）
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(secs))
            .no_proxy()
            .build()
            .expect("Failed to create HTTP client");
        self
    }
}

impl LLMProvider for DeepSeekProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LlmResponse, LlmError> {
        let request_body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            stream: false,
        };

        let response = self.client
            .post("https://api.deepseek.com/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
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

        let choice = chat_resp.choices.into_iter().next()
            .ok_or_else(|| LlmError::Api("No choices returned".into()))?;

        let tool_calls = choice.message.tool_calls.map(|calls| {
            calls.into_iter().map(|tc| crate::ToolCall {
                id: tc.id,
                call_type: tc.call_type,
                function: crate::ToolCallFunction {
                    name: tc.function.name,
                    arguments: tc.function.arguments,
                },
            }).collect()
        }).unwrap_or_default();

        Ok(LlmResponse {
            content: choice.message.content.unwrap_or_default(),
            tool_calls,
            finish_reason: choice.finish_reason,
            usage: chat_resp.usage.map(|u| crate::Usage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            }),
        })
    }

    fn chat_stream(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Box<dyn StreamIterator>, LlmError> {
        // DeepSeek 不支持 stream + tools 同时使用
        // 有工具时 fallback 到非 streaming 模式，把结果包装成单 chunk stream
        if !tools.is_empty() {
            let response = self.chat(messages, tools)?;
            return Ok(Box::new(crate::SingleChunkStream::new(response)));
        }

        let request_body = ChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            tools: tools.to_vec(),
            stream: true,
        };

        let response = self.client
            .post("https://api.deepseek.com/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&request_body)
            .send()
            .map_err(|e| LlmError::Http(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(LlmError::Api(format!("HTTP {}: {}", status, body)));
        }

        Ok(Box::new(DeepSeekStream {
            reader: BufReader::new(response),
            buffer: String::new(),
            done: false,
        }))
    }

    fn models(&self) -> Vec<String> {
        vec![self.model.clone()]
    }
}

/// DeepSeek SSE 流式读取器（与 OpenAI 格式相同）
pub struct DeepSeekStream {
    reader: BufReader<reqwest::blocking::Response>,
    buffer: String,
    done: bool,
}

impl StreamIterator for DeepSeekStream {
    fn next_chunk(&mut self) -> Option<Result<StreamChunk, LlmError>> {
        if self.done {
            return None;
        }

        loop {
            self.buffer.clear();
            match self.reader.read_line(&mut self.buffer) {
                Ok(0) => {
                    self.done = true;
                    return None;
                }
                Ok(_) => {
                    let line = self.buffer.trim();
                    if line.is_empty() {
                        continue;
                    }
                    if line == "data: [DONE]" {
                        self.done = true;
                        return Some(Ok(StreamChunk {
                            content: String::new(),
                            tool_calls: Vec::new(),
                            finish_reason: Some("stop".into()),
                        }));
                    }
                    if let Some(data) = line.strip_prefix("data: ") {
                        match serde_json::from_str::<StreamDelta>(data) {
                            Ok(delta) => {
                                let content = delta.choices.first()
                                    .and_then(|c| c.delta.content.clone())
                                    .unwrap_or_default();
                                let finish_reason = delta.choices.first()
                                    .and_then(|c| c.finish_reason.clone());

                                return Some(Ok(StreamChunk { content, tool_calls: Vec::new(), finish_reason }));
                            }
                            Err(e) => {
                                return Some(Err(LlmError::Parse(format!("SSE parse error: {} line: {}", e, data))));
                            }
                        }
                    }
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(LlmError::Http(e.to_string())));
                }
            }
        }
    }
}
