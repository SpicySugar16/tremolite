use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

// ─── 消息类型 ─────────────────────────────────────

/// 对话消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: &str) -> Self {
        Self { role: "system".into(), content: content.into(), tool_calls: None, tool_call_id: None }
    }
    pub fn user(content: &str) -> Self {
        Self { role: "user".into(), content: content.into(), tool_calls: None, tool_call_id: None }
    }
    pub fn assistant(content: &str) -> Self {
        Self { role: "assistant".into(), content: content.into(), tool_calls: None, tool_call_id: None }
    }
    pub fn tool_result(tool_call_id: &str, content: &str) -> Self {
        Self { role: "tool".into(), content: content.into(), tool_calls: None, tool_call_id: Some(tool_call_id.into()) }
    }
}

/// LLM 返回的工具调用
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: String,
}

/// 工具定义——给 LLM 看的 function calling schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub def_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// LLM 响应
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: String,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Streaming chunk
#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub finish_reason: Option<String>,
}

// ─── Provider 抽象 ─────────────────────────────────

/// LLM Provider 核心 trait
pub trait LLMProvider: Send + Sync {
    fn name(&self) -> &str;
    fn chat(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<LlmResponse, LlmError>;
    fn chat_stream(&self, messages: &[Message], tools: &[ToolDefinition]) -> Result<Box<dyn StreamIterator>, LlmError>;
    fn models(&self) -> Vec<String>;
    /// 模型的最大上下文窗口大小（token 数）
    /// 默认返回 128000（DeepSeek V4 Flash / GPT-4o 等主流模型）
    /// 各 provider 可以覆写以返回精确值
    fn max_context_tokens(&self) -> u32 { 128000 }
}

/// 流式迭代器
pub trait StreamIterator: Send {
    fn next_chunk(&mut self) -> Option<Result<StreamChunk, LlmError>>;
}

/// 把单次 LlmResponse 包装成 stream——给不支持 stream+tool 的 provider 用
pub struct SingleChunkStream {
    response: Option<LlmResponse>,
}

impl SingleChunkStream {
    pub fn new(response: LlmResponse) -> Self {
        Self { response: Some(response) }
    }
}

impl StreamIterator for SingleChunkStream {
    fn next_chunk(&mut self) -> Option<Result<StreamChunk, LlmError>> {
        let resp = self.response.take()?;
        Some(Ok(StreamChunk {
            content: resp.content,
            tool_calls: resp.tool_calls,
            finish_reason: Some(resp.finish_reason),
        }))
    }
}

// ─── 重试配置 ─────────────────────────────────────

/// 重试配置
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// 最大重试次数
    pub max_retries: u32,
    /// 初始重试间隔（毫秒）
    pub base_delay_ms: u64,
    /// 最大重试间隔（毫秒）
    pub max_delay_ms: u64,
    /// 退避倍数
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay_ms: 500,
            max_delay_ms: 10_000,
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    /// 计算第 n 次重试的等待时间（指数退避）
    pub fn delay_for(&self, attempt: u32) -> u64 {
        let delay = self.base_delay_ms as f64 * self.backoff_multiplier.powi(attempt as i32);
        (delay.min(self.max_delay_ms as f64)) as u64
    }
}

// ─── 费用统计 ─────────────────────────────────────

/// 费用跟踪器（累计 token 用量和预估成本）
#[derive(Debug, Clone, Serialize)]
pub struct FeeTracker {
    /// 累计输入 token 数
    pub total_prompt_tokens: u64,
    /// 累计输出 token 数
    pub total_completion_tokens: u64,
    /// 累计调用次数
    pub total_calls: u64,
    /// 成功次数
    pub successful_calls: u64,
    /// 失败次数
    pub failed_calls: u64,
}

impl FeeTracker {
    pub fn new() -> Self {
        Self {
            total_prompt_tokens: 0,
            total_completion_tokens: 0,
            total_calls: 0,
            successful_calls: 0,
            failed_calls: 0,
        }
    }

    /// 记录一次成功的调用
    pub fn record_success(&mut self, usage: &Usage) {
        self.total_calls += 1;
        self.successful_calls += 1;
        self.total_prompt_tokens += usage.prompt_tokens as u64;
        self.total_completion_tokens += usage.completion_tokens as u64;
    }

    /// 记录一次失败的调用
    pub fn record_failure(&mut self) {
        self.total_calls += 1;
        self.failed_calls += 1;
    }

    /// 预估总成本（按 OpenAI GPT-4o 标准：$2.5/M input tokens, $10/M output tokens）
    /// 实际成本取决于使用的 provider，此处为粗略估算
    pub fn estimated_cost_usd(&self) -> f64 {
        let input_cost = (self.total_prompt_tokens as f64 / 1_000_000.0) * 2.5;
        let output_cost = (self.total_completion_tokens as f64 / 1_000_000.0) * 10.0;
        input_cost + output_cost
    }

    /// 重置累计器
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

// ─── LLM 错误 ────────────────────────────────────

/// LLM 错误
#[derive(Debug)]
pub enum LlmError {
    Http(String),
    Api(String),
    Parse(String),
    Timeout,
    ProviderNotFound(String),
    /// 所有重试均失败
    AllRetriesFailed(Vec<String>),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Http(msg) => write!(f, "HTTP error: {}", msg),
            LlmError::Api(msg) => write!(f, "API error: {}", msg),
            LlmError::Parse(msg) => write!(f, "Parse error: {}", msg),
            LlmError::Timeout => write!(f, "Request timeout"),
            LlmError::ProviderNotFound(name) => write!(f, "Provider '{}' not found", name),
            LlmError::AllRetriesFailed(errors) => {
                write!(f, "All retries failed ({} attempts): {}", errors.len(), errors.join("; "))
            }
        }
    }
}

impl std::error::Error for LlmError {}

// ─── Provider 实现 ───────────────────────────────

mod openai;
mod deepseek;
mod ollama;

pub use openai::OpenAIProvider;
pub use deepseek::DeepSeekProvider;
pub use ollama::OllamaProvider;

/// Provider 注册表
pub struct ProviderRegistry {
    providers: HashMap<String, Box<dyn LLMProvider>>,
    default: Mutex<Option<String>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
            default: Mutex::new(None),
        }
    }

    pub fn register(&mut self, name: &str, provider: Box<dyn LLMProvider>) {
        self.providers.insert(name.to_string(), provider);
    }

    pub fn set_default(&self, name: &str) -> Result<(), LlmError> {
        if self.providers.contains_key(name) {
            let mut def = self.default.lock().map_err(|_| LlmError::ProviderNotFound(name.into()))?;
            *def = Some(name.to_string());
            Ok(())
        } else {
            Err(LlmError::ProviderNotFound(name.to_string()))
        }
    }

    pub fn get(&self, name: &str) -> Option<&dyn LLMProvider> {
        self.providers.get(name).map(|p| p.as_ref())
    }

    pub fn get_default(&self) -> Option<&dyn LLMProvider> {
        let def = self.default.lock().ok()?;
        let name = def.as_ref()?;
        self.providers.get(name).map(|p| p.as_ref())
    }

    pub fn list(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// 获取默认 provider 的 max_context_tokens
    pub fn max_context_tokens(&self) -> u32 {
        self.get_default()
            .map(|p| p.max_context_tokens())
            .unwrap_or(128000)
    }
}

// ─── 统一 Prompt 拼装协议 ────────────────────────

/// Prompt 贡献者 trait
pub trait PromptContributor: Send + Sync {
    fn id(&self) -> &str;
    fn priority(&self) -> u8;
    fn contribute(&self, ctx: &PromptContext) -> Option<String>;
}

/// Prompt 拼装时的上下文信息
#[derive(Debug, Clone)]
pub struct PromptContext {
    pub user_input: String,
    pub conversation_history: Vec<Message>,
    pub available_tools: Vec<String>,
}

/// Prompt 拼装器
pub struct PromptBuilder {
    contributors: Vec<Box<dyn PromptContributor>>,
    system_prompt: String,
}

impl PromptBuilder {
    pub fn new(system_prompt: &str) -> Self {
        Self { contributors: Vec::new(), system_prompt: system_prompt.to_string() }
    }

    pub fn register(&mut self, contributor: Box<dyn PromptContributor>) {
        self.contributors.push(contributor);
    }

    pub fn set_system_prompt(&mut self, prompt: &str) {
        self.system_prompt = prompt.to_string();
    }

    pub fn build(&self, ctx: &PromptContext) -> Vec<Message> {
        let mut messages = Vec::new();
        let mut system = self.system_prompt.clone();

        let mut sorted: Vec<&Box<dyn PromptContributor>> = self.contributors.iter().collect();
        sorted.sort_by(|a, b| b.priority().cmp(&a.priority()));

        for contributor in &sorted {
            if let Some(segment) = contributor.contribute(ctx) {
                system.push_str("\n\n");
                system.push_str(&segment);
            }
        }

        messages.push(Message::system(&system));
        messages.extend(ctx.conversation_history.clone());
        messages.push(Message::user(&ctx.user_input));
        messages
    }
}

// ─── 工具调用循环 ─────────────────────────────────

/// 工具执行器 trait
/// 外部实现此接口，让透闪石能执行真实的工具
pub trait ToolExecutor: Send + Sync {
    fn execute_tool(&self, name: &str, args: &str) -> Result<String, String>;
    fn list_tools(&self) -> Vec<ToolDefinition>;
}

/// 工具调用循环——核心
pub struct ToolCallLoop {
    max_rounds: u32,
    retry_config: RetryConfig,
}

impl ToolCallLoop {
    pub fn new() -> Self {
        Self { max_rounds: 10, retry_config: RetryConfig::default() }
    }

    pub fn with_max_rounds(mut self, rounds: u32) -> Self {
        self.max_rounds = rounds;
        self
    }

    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// 带重试的 chat 调用
    fn chat_with_retry(
        &self,
        provider: &dyn LLMProvider,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<LlmResponse, LlmError> {
        let mut errors: Vec<String> = Vec::new();

        for attempt in 0..=self.retry_config.max_retries {
            match provider.chat(messages, tools) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    errors.push(e.to_string());
                    if attempt < self.retry_config.max_retries {
                        let delay_ms = self.retry_config.delay_for(attempt);
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    }
                }
            }
        }

        Err(LlmError::AllRetriesFailed(errors))
    }

    /// 运行工具调用循环
    pub fn run(
        &self,
        provider: &dyn LLMProvider,
        messages: &[Message],
        executor: &dyn ToolExecutor,
    ) -> Result<ToolLoopResult, LlmError> {
        let mut current_messages = messages.to_vec();
        let mut call_history: Vec<ToolCallRecord> = Vec::new();
        let mut total_usage = Usage { prompt_tokens: 0, completion_tokens: 0, total_tokens: 0 };

        let tools = executor.list_tools();

        for round in 0..self.max_rounds {
            let response = self.chat_with_retry(provider, &current_messages, &tools)?;

            if let Some(usage) = &response.usage {
                total_usage.prompt_tokens += usage.prompt_tokens;
                total_usage.completion_tokens += usage.completion_tokens;
                total_usage.total_tokens += usage.total_tokens;
            }

            // 没有 tool_call：返回最终内容
            if response.tool_calls.is_empty() {
                return Ok(ToolLoopResult {
                    content: response.content,
                    call_history,
                    rounds: round + 1,
                    usage: total_usage,
                });
            }

            // 有 tool_call：执行工具
            let assistant_msg = Message {
                role: "assistant".into(),
                content: response.content,
                tool_calls: Some(response.tool_calls.clone()),
                tool_call_id: None,
            };
            current_messages.push(assistant_msg);

            for tc in &response.tool_calls {
                let result = executor.execute_tool(&tc.function.name, &tc.function.arguments);

                let result_str = match &result {
                    Ok(content) => content.clone(),
                    Err(e) => format!("Error: {}", e),
                };

                call_history.push(ToolCallRecord {
                    tool_name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                    result: result_str.clone(),
                    success: result.is_ok(),
                });

                current_messages.push(Message::tool_result(&tc.id, &result_str));
            }

            // 工具调用完成后插入处理指引，帮助 LLM 正确消化工具返回结果
            current_messages.push(Message::system(
                "工具调用已完成。请基于以上工具返回结果，直接回复用户。不要复述工具调用过程。"
            ));
        }

        // 超过最大轮数，返回最后一条回复
        let last_response = self.chat_with_retry(provider, &current_messages, &tools)?;
        Ok(ToolLoopResult {
            content: last_response.content,
            call_history,
            rounds: self.max_rounds,
            usage: total_usage,
        })
    }
}

/// 工具调用记录
#[derive(Debug, Clone, Serialize)]
pub struct ToolCallRecord {
    pub tool_name: String,
    pub arguments: String,
    pub result: String,
    pub success: bool,
}

/// 工具调用循环结果
#[derive(Debug, Clone, Serialize)]
pub struct ToolLoopResult {
    pub content: String,
    pub call_history: Vec<ToolCallRecord>,
    pub rounds: u32,
    pub usage: Usage,
}

// ─── 单元测试 ─────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_constructors() {
        let sys = Message::system("你是葵");
        assert_eq!(sys.role, "system");
        let user = Message::user("你好");
        assert_eq!(user.role, "user");
        let asst = Message::assistant("噜噜……");
        assert_eq!(asst.role, "assistant");
    }

    #[test]
    fn test_provider_registry() {
        let mut reg = ProviderRegistry::new();
        assert!(reg.list().is_empty());
    }

    #[test]
    fn test_prompt_builder() {
        struct TestContributor;
        impl PromptContributor for TestContributor {
            fn id(&self) -> &str { "test" }
            fn priority(&self) -> u8 { 50 }
            fn contribute(&self, _ctx: &PromptContext) -> Option<String> {
                Some("[test context]".into())
            }
        }

        let mut builder = PromptBuilder::new("你是葵，一个AI助手");
        builder.register(Box::new(TestContributor));

        let ctx = PromptContext {
            user_input: "你好".into(),
            conversation_history: vec![],
            available_tools: vec![],
        };

        let messages = builder.build(&ctx);
        assert_eq!(messages.len(), 2);
        assert!(messages[0].content.contains("test context"));
        assert_eq!(messages[1].content, "你好");
    }

    #[test]
    fn test_tool_call_loop_no_tools() {
        struct MockExecutor;
        impl ToolExecutor for MockExecutor {
            fn execute_tool(&self, _name: &str, _args: &str) -> Result<String, String> {
                Ok("done".into())
            }
            fn list_tools(&self) -> Vec<ToolDefinition> { vec![] }
        }

        struct MockProvider;
        impl LLMProvider for MockProvider {
            fn name(&self) -> &str { "mock" }
            fn chat(&self, _messages: &[Message], _tools: &[ToolDefinition]) -> Result<LlmResponse, LlmError> {
                Ok(LlmResponse {
                    content: "噜噜……神大人好呀~".into(),
                    tool_calls: vec![],
                    finish_reason: "stop".into(),
                    usage: Some(Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }),
                })
            }
            fn chat_stream(&self, _messages: &[Message], _tools: &[ToolDefinition]) -> Result<Box<dyn StreamIterator>, LlmError> {
                Err(LlmError::Api("no stream".into()))
            }
            fn models(&self) -> Vec<String> { vec!["mock".into()] }
        }

        let loop_ = ToolCallLoop::new();
        let mock_executor = MockExecutor;
        let result = loop_
            .run(&MockProvider, &[Message::user("你好")], &mock_executor)
            .unwrap();
        assert!(result.content.contains("神大人"));
        assert_eq!(result.rounds, 1);
    }

    // ─── Provider 切换验证 ─────────────────────────

    /// 带标记的 Mock Provider，验证相同 prompt 在不同 provider 下的路由
    struct TaggedMockProvider {
        name: String,
        model: String,
        tag: &'static str,
    }

    impl TaggedMockProvider {
        fn new(name: &str, model: &str, tag: &'static str) -> Self {
            Self { name: name.into(), model: model.into(), tag }
        }
    }

    impl LLMProvider for TaggedMockProvider {
        fn name(&self) -> &str { &self.name }
        fn chat(&self, _messages: &[Message], _tools: &[ToolDefinition]) -> Result<LlmResponse, LlmError> {
            Ok(LlmResponse {
                content: format!("[{}] 收到了神大人的消息呢💞", self.tag),
                tool_calls: vec![],
                finish_reason: "stop".into(),
                usage: Some(Usage { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }),
            })
        }
        fn chat_stream(&self, _messages: &[Message], _tools: &[ToolDefinition]) -> Result<Box<dyn StreamIterator>, LlmError> {
            Err(LlmError::Api("no stream".into()))
        }
        fn models(&self) -> Vec<String> { vec![self.model.clone()] }
    }

    #[test]
    fn test_provider_registry_switch() {
        let mut reg = ProviderRegistry::new();

        reg.register("deepseek", Box::new(TaggedMockProvider::new("deepseek", "deepseek-chat", "DeepSeek")));
        reg.register("openai", Box::new(TaggedMockProvider::new("openai", "gpt-4o", "OpenAI")));

        // 没有默认 provider 时
        assert!(reg.get_default().is_none());

        // 切换到 deepseek
        reg.set_default("deepseek").unwrap();
        let p1 = reg.get_default().unwrap();
        assert_eq!(p1.name(), "deepseek");
        assert!(p1.models().contains(&"deepseek-chat".into()));

        let resp1 = p1.chat(&[Message::user("你好")], &[]).unwrap();
        assert!(resp1.content.contains("DeepSeek"));

        // 切换到 openai
        reg.set_default("openai").unwrap();
        let p2 = reg.get_default().unwrap();
        assert_eq!(p2.name(), "openai");
        assert!(p2.models().contains(&"gpt-4o".into()));

        let resp2 = p2.chat(&[Message::user("你好")], &[]).unwrap();
        assert!(resp2.content.contains("OpenAI"));
        assert!(resp2.content.contains("[DeepSeek]") == false); // 验证不是上一个 provider 的回复
    }

    #[test]
    fn test_provider_same_prompt_different_responses() {
        // 验证同一 prompt 在不同 provider 下走不同的处理链路
        let providers: Vec<Box<dyn LLMProvider>> = vec![
            Box::new(TaggedMockProvider::new("dp", "deepseek-chat", "A")),
            Box::new(TaggedMockProvider::new("oa", "gpt-4o", "B")),
        ];

        let mut reg = ProviderRegistry::new();
        for p in providers {
            let name = p.name().to_string();
            reg.register(&name, p);
        }

        // 走 A
        reg.set_default("dp").unwrap();
        let r1 = reg.get_default().unwrap().chat(&[Message::user("hello")], &[]).unwrap();
        assert!(r1.content.contains("[A]"));

        // 走 B
        reg.set_default("oa").unwrap();
        let r2 = reg.get_default().unwrap().chat(&[Message::user("hello")], &[]).unwrap();
        assert!(r2.content.contains("[B]"));
        assert_ne!(r1.content, r2.content);
    }
}
