use std::any::Any;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tremolite_llm::{ToolDefinition, ToolFunction};

use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// Webhook 配置——一个外部事件源对应一个 webhook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// webhook 名称（也用作路由 PATH：/webhooks/<name>）
    pub name: String,
    /// 来源标识（如 github、gitlab、custom）
    pub source: String,
    /// 触发条件——事件属性匹配规则（JSON Path 风格）
    #[serde(default)]
    pub conditions: Vec<ConditionRule>,
    /// 触发后的动作
    pub action: WebhookAction,
    /// 最近一次触发时间
    #[serde(default)]
    pub last_triggered: Option<u64>,
    /// 触发次数
    #[serde(default)]
    pub trigger_count: u64,
}

/// 条件规则
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConditionRule {
    /// 要匹配的字段路径（如 "action"、"pull_request.state"）
    pub field: String,
    /// 预期的值
    pub value: String,
    /// 匹配操作符（eq/contains/prefix）
    #[serde(default = "default_operator")]
    pub operator: String,
}

fn default_operator() -> String { "eq".into() }

/// 触发后的动作
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WebhookAction {
    /// 调用一个内置工具
    #[serde(rename = "tool")]
    Tool { tool: String, args: HashMap<String, String> },
    /// 用 LLM 处理事件 payload
    #[serde(rename = "llm_prompt")]
    LlmPrompt { prompt_template: String },
    /// 记录日志
    #[serde(rename = "log")]
    Log { message: String },
}

/// Webhook 接收到的外部事件
#[derive(Debug, Clone)]
pub struct WebhookEvent {
    pub name: String,
    pub source: String,
    pub headers: HashMap<String, String>,
    pub payload: serde_json::Value,
}

/// Webhook 模块——监听外部事件，触发自动化流水线
pub struct WebhookModule {
    hooks: Vec<WebhookConfig>,
    event_log: Vec<(String, u64, bool)>,
}

impl WebhookModule {
    pub fn new() -> Self {
        Self {
            hooks: Vec::new(),
            event_log: Vec::new(),
        }
    }

    /// 注册一条 webhook
    pub fn register(&mut self, config: WebhookConfig) -> Result<(), String> {
        if self.hooks.iter().any(|h| h.name == config.name) {
            return Err(format!("webhook '{}' already registered", config.name));
        }
        tracing::info!("webhook: registered '{}' (source: {})", config.name, config.source);
        self.hooks.push(config);
        Ok(())
    }

    /// 注销一条 webhook
    pub fn unregister(&mut self, name: &str) -> bool {
        let len = self.hooks.len();
        self.hooks.retain(|h| h.name != name);
        let removed = self.hooks.len() < len;
        if removed {
            tracing::info!("webhook: unregistered '{}'", name);
        }
        removed
    }

    /// 获取所有 webhook 列表
    pub fn list(&self) -> &[WebhookConfig] {
        &self.hooks
    }

    /// 按名称查找 webhook
    pub fn get(&self, name: &str) -> Option<&WebhookConfig> {
        self.hooks.iter().find(|h| h.name == name)
    }

    /// 接收外部事件，匹配并触发动作
    pub fn receive(&mut self, event: WebhookEvent) -> Result<String, String> {
        // 先取出需要的信息，避免可变借用和不可变借用冲突
    let hook_idx = self.hooks.iter().position(|h| h.name == event.name)
        .ok_or_else(|| format!("webhook '{}' not found", event.name))?;
    let conditions = self.hooks[hook_idx].conditions.clone();
    let source = self.hooks[hook_idx].source.clone();
    let triggered = current_secs();

    // 检查来源是否匹配
    if source != event.source {
        return Err(format!(
            "source mismatch: expected '{}', got '{}'",
            source, event.source
        ));
    }

    // 检查条件（此时 self 没有可变借用）
    if !self.check_conditions(&conditions, &event.payload) {
        tracing::info!("webhook '{}': conditions not met, skipped", event.name);
        self.event_log.push((event.name.clone(), triggered, false));
        return Ok("conditions not met, skipped".into());
    }

    // 触发动作
    let hook = &mut self.hooks[hook_idx];
    hook.last_triggered = Some(triggered);
    hook.trigger_count += 1;
    self.event_log.push((event.name.clone(), triggered, true));

    let result = match &hook.action {
            WebhookAction::Log { message } => {
                tracing::info!("webhook '{}': {}", event.name, message);
                format!("[logged] {}", message)
            }
            WebhookAction::Tool { tool, args } => {
                tracing::info!("webhook '{}': triggering tool '{}'", event.name, tool);
                format!("[triggered] tool '{}' with {:?}", tool, args)
            }
            WebhookAction::LlmPrompt { prompt_template } => {
                let filled = prompt_template
                    .replace("{{event}}", &event.payload.to_string())
                    .replace("{{source}}", &event.source);
                tracing::info!("webhook '{}': scheduled LLM prompt", event.name);
                format!("[scheduled] prompt: {}", &filled[..filled.len().min(100)])
            }
        };

        tracing::info!("webhook '{}': triggered successfully", event.name);
        Ok(result)
    }

    /// 检查事件 payload 是否满足所有条件
    fn check_conditions(&self, conditions: &[ConditionRule], payload: &serde_json::Value) -> bool {
        if conditions.is_empty() {
            return true; // 无条件 = 全部通过
        }
        conditions.iter().all(|rule| {
            let value = resolve_json_path(payload, &rule.field);
            match value {
                Some(v) => {
                    let v_str = v.as_str().map(|s| s.to_string())
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                        .or_else(|| v.as_bool().map(|b| b.to_string()))
                        .unwrap_or_default();
                    match rule.operator.as_str() {
                        "eq" => v_str == rule.value,
                        "contains" => v_str.contains(&rule.value),
                        "prefix" => v_str.starts_with(&rule.value),
                        _ => false,
                    }
                }
                None => false,
            }
        })
    }

    /// 获取事件日志（最近 20 条）
    pub fn event_log(&self) -> &[(String, u64, bool)] {
        &self.event_log
    }
}

/// 按点分割的 JSON Path 解析值
fn resolve_json_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;
    for part in parts {
        match current {
            serde_json::Value::Object(map) => {
                current = map.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn current_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ─── Module trait 实现 ─────────────────────────

impl Module for WebhookModule {
    fn id(&self) -> &str { "webhook" }
    fn name(&self) -> &str { "Webhook 订阅" }
    fn version(&self) -> &str { "0.1.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "webhook.receive".into(),
            "webhook.manage".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "register_webhook".into(),
                    description: "注册一条 webhook——外部事件源触发时自动执行动作".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "webhook 名称（唯一）" },
                            "source": { "type": "string", "description": "来源（如 github、custom）" },
                            "action_type": { "type": "string", "enum": ["log", "tool", "llm_prompt"], "description": "触发动作类型" },
                            "tool_name": { "type": "string", "description": "action_type=tool 时，要调用的工具名" },
                            "prompt_template": { "type": "string", "description": "action_type=llm_prompt 时，提示模板（支持 {{event}} {{source}}）" },
                            "condition_field": { "type": "string", "description": "条件字段路径，如 action" },
                            "condition_value": { "type": "string", "description": "条件字段预期值" },
                        },
                        "required": ["name", "source", "action_type"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "list_webhooks".into(),
                    description: "列出所有注册的 webhook".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "delete_webhook".into(),
                    description: "删除一条 webhook".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "要删除的 webhook 名称" }
                        },
                        "required": ["name"]
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "register_webhook" => self.cmd_register(args),
            "list_webhooks" => self.cmd_list(),
            "delete_webhook" => self.cmd_delete(args),
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                tracing::info!("webhook: module ready ({} hooks configured)", self.hooks.len());
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { Some(self) }
}

// ─── 工具命令处理 ─────────────────────────────

impl WebhookModule {
    fn cmd_register(&mut self, args: &str) -> Result<String, ModuleError> {
        let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(args)
            .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析: {e}")))?;

        let name = parsed.get("name")
            .and_then(|v| v.as_str()).unwrap_or("").to_string();
        let source = parsed.get("source")
            .and_then(|v| v.as_str()).unwrap_or("custom").to_string();

        if name.is_empty() {
            return Err(ModuleError::ToolExecutionFailed("name 不能为空".into()));
        }

        let action_type = parsed.get("action_type")
            .and_then(|v| v.as_str()).unwrap_or("log");

        let action = match action_type {
            "tool" => {
                let tool = parsed.get("tool_name")
                    .and_then(|v| v.as_str()).unwrap_or("")
                    .to_string();
                WebhookAction::Tool { tool, args: HashMap::new() }
            }
            "llm_prompt" => {
                let prompt_template = parsed.get("prompt_template")
                    .and_then(|v| v.as_str()).unwrap_or("收到 {{source}} 事件")
                    .to_string();
                WebhookAction::LlmPrompt { prompt_template }
            }
            _ => WebhookAction::Log { message: format!("webhook '{}' triggered", name) },
        };

        // 条件
        let mut conditions = Vec::new();
        if let (Some(field), Some(value)) = (
            parsed.get("condition_field").and_then(|v| v.as_str()),
            parsed.get("condition_value").and_then(|v| v.as_str()),
        ) {
            conditions.push(ConditionRule {
                field: field.to_string(),
                value: value.to_string(),
                operator: "eq".into(),
            });
        }

        let config = WebhookConfig {
            name: name.clone(),
            source,
            conditions,
            action,
            last_triggered: None,
            trigger_count: 0,
        };

        self.register(config)
            .map_err(|e| ModuleError::ToolExecutionFailed(e))?;

        Ok(format!("webhook '{}' 已注册，POST /webhooks/{} 来触发它呢~", name, name))
    }

    fn cmd_list(&self) -> Result<String, ModuleError> {
        if self.hooks.is_empty() {
            return Ok("还没有注册任何 webhook 呢~".into());
        }
        let lines: Vec<String> = self.hooks.iter().map(|h| {
            let last = h.last_triggered.map(|t| format!("上次触发: {}", t)).unwrap_or_else(|| "从未触发".into());
            format!("  [{}] {} — {} ({} 次, {})", h.source, h.name, action_summary(&h.action), h.trigger_count, last)
        }).collect();
        Ok(format!("已注册的 webhook ({}):\n{}", self.hooks.len(), lines.join("\n")))
    }

    fn cmd_delete(&mut self, args: &str) -> Result<String, ModuleError> {
        let parsed: HashMap<String, String> = serde_json::from_str(args)
            .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析: {e}")))?;
        let name = parsed.get("name").map(|s| s.as_str()).unwrap_or("");
        if self.unregister(name) {
            Ok(format!("webhook '{}' 已删除", name))
        } else {
            Err(ModuleError::ToolExecutionFailed(format!("webhook '{}' 未找到", name)))
        }
    }
}

fn action_summary(action: &WebhookAction) -> String {
    match action {
        WebhookAction::Log { .. } => "记录日志".into(),
        WebhookAction::Tool { tool, .. } => format!("调用工具 '{}'", tool),
        WebhookAction::LlmPrompt { .. } => "LLM 处理".into(),
    }
}
