use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── JSON-RPC 2.0 消息 ──────────────────────────

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
    #[serde(default)]
    data: Option<serde_json::Value>,
}

// ─── MCP 数据类型 ──────────────────────────────

/// 来自 MCP 服务器的工具声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// MCP 资源声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceDef {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub mime_type: Option<String>,
}

/// MCP 提示声明
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub arguments: Vec<McpPromptArg>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPromptArg {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

/// 工具调用结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: bool,
}

/// MCP 内容块
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mime_type: String },
    #[serde(rename = "resource")]
    Resource { resource: McpResourceRef },
}

/// MCP 资源引用（作为内容嵌入）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceRef {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,
}

/// MCP 资源内容（resources/read 返回）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResourceContents {
    pub uri: String,
    pub mime_type: Option<String>,
    pub text: Option<String>,
    pub blob: Option<String>,
}

// ─── 传输类型 ────────────────────────────────

/// MCP 传输方式
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "transport")]
pub enum TransportConfig {
    /// HTTP JSON-RPC（默认）
    #[serde(rename = "http")]
    Http {
        url: String,
    },
    /// SSE（Server-Sent Events）
    #[serde(rename = "sse")]
    Sse {
        url: String,
    },
    /// Stdio 子进程
    #[serde(rename = "stdio")]
    Stdio {
        command: String,
        args: Vec<String>,
    },
}

// ─── MCP 客户端配置 ──────────────────────────

/// MCP 客户端配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub transport: TransportConfig,
    #[serde(default)]
    pub prefix: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

fn default_timeout() -> u64 { 30 }

// ─── MCP 客户端 ────────────────────────────────

/// MCP 客户端——与一个 MCP 服务通信
pub struct McpClient {
    config: McpServerConfig,
    seq: u64,
    /// 已发现的工具缓存
    cached_tools: Vec<McpTool>,
    /// 已发现的资源缓存
    cached_resources: Vec<McpResourceDef>,
    /// 已发现的提示缓存
    cached_prompts: Vec<McpPromptDef>,
    /// Stdio 子进程（仅 stdio 模式使用）
    stdio_child: Option<std::process::Child>,
}

impl McpClient {
    pub fn new(config: McpServerConfig) -> Self {
        Self {
            config,
            seq: 1,
            cached_tools: Vec::new(),
            cached_resources: Vec::new(),
            cached_prompts: Vec::new(),
            stdio_child: None,
        }
    }

    // ── HTTP 传输 ──────────────────────────────

    fn send_request_http(&mut self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
        let url = match &self.config.transport {
            TransportConfig::Http { url } => url.clone(),
            TransportConfig::Sse { .. } => {
                // SSE 模式下也通过 HTTP POST 发送请求（message endpoint）
                // 不过 SSE 的 message endpoint 需要从 SSE 事件中发现
                // 这里先用配置中的 url + "/message"
                format!("{}/message", self.config.transport.url())
            }
            _ => return Err("http transport not available for this client".into()),
        };

        let id = self.seq;
        self.seq += 1;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        };
        let body = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;
        let timeout = std::time::Duration::from_secs(self.config.timeout_secs);

        tracing::debug!("mcp: POST {url} method={method}");

        let response = ureq::post(&url)
            .set("Content-Type", "application/json")
            .timeout(timeout)
            .send_string(&body)
            .map_err(|e| format!("mcp: HTTP request to {url} failed: {e}"))?;

        let resp_body = response
            .into_string()
            .map_err(|e| format!("mcp: read response: {e}"))?;

        let rpc_resp: JsonRpcResponse = serde_json::from_str(&resp_body)
            .map_err(|e| format!("mcp: parse response: {e} (body: {resp_body})"))?;

        if let Some(err) = rpc_resp.error {
            return Err(format!("mcp: {method} error (code={}): {}", err.code, err.message));
        }
        rpc_resp.result.ok_or_else(|| "mcp: empty response".into())
    }

    // ── Stdio 传输 ─────────────────────────────

    fn spawn_stdio(&mut self) -> Result<(), String> {
        let (command, args) = match &self.config.transport {
            TransportConfig::Stdio { command, args } => (command.clone(), args.clone()),
            _ => return Ok(()),
        };

        let child = std::process::Command::new(&command)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .map_err(|e| format!("mcp: failed to spawn {command}: {e}"))?;

        self.stdio_child = Some(child);
        Ok(())
    }

    fn send_request_stdio(&mut self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
        let child = self.stdio_child.as_mut()
            .ok_or_else(|| "mcp: stdio child not spawned".to_string())?;

        let stdin = child.stdin.as_mut()
            .ok_or_else(|| "mcp: stdin not available".to_string())?;
        let stdout = child.stdout.as_mut()
            .ok_or_else(|| "mcp: stdout not available".to_string())?;

        let id = self.seq;
        self.seq += 1;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id,
            method: method.into(),
            params,
        };
        let line = serde_json::to_string(&req).map_err(|e| format!("serialize: {e}"))?;

        use std::io::Write;
        writeln!(stdin, "{line}").map_err(|e| format!("mcp: write to stdin: {e}"))?;
        stdin.flush().ok();

        // 读一行响应
        let mut resp_line = String::new();
        use std::io::BufRead;
        let mut reader = std::io::BufReader::new(stdout);
        reader.read_line(&mut resp_line)
            .map_err(|e| format!("mcp: read from stdout: {e}"))?;

        let rpc_resp: JsonRpcResponse = serde_json::from_str(&resp_line)
            .map_err(|e| format!("mcp: parse response: {e} (line: {resp_line})"))?;

        if let Some(err) = rpc_resp.error {
            return Err(format!("mcp: {method} error (code={}): {}", err.code, err.message));
        }
        rpc_resp.result.ok_or_else(|| "mcp: empty response".into())
    }

    // ── 通用请求 ─────────────────────────────

    fn send_request(&mut self, method: &str, params: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
        match &self.config.transport {
            TransportConfig::Stdio { .. } => self.send_request_stdio(method, params),
            _ => self.send_request_http(method, params),
        }
    }

    // ── 工具发现和调用 ─────────────────────────

    /// 发现 MCP 服务器上的工具
    pub fn discover_tools(&mut self) -> Result<Vec<McpTool>, String> {
        let result = self.send_request("tools/list", None)?;
        let tools: Vec<McpTool> = result
            .get("tools")
            .and_then(|v| serde_json::from_value::<Vec<McpTool>>(v.clone()).ok())
            .unwrap_or_default();
        self.cached_tools = tools.clone();
        tracing::info!("mcp: discovered {} tools from '{}'", tools.len(), self.config.name);
        Ok(tools)
    }

    /// 调用一个工具
    pub fn call_tool(&mut self, tool_name: &str, arguments: serde_json::Value) -> Result<McpToolResult, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });
        let result = self.send_request("tools/call", Some(params))?;
        let tool_result: McpToolResult = serde_json::from_value(result)
            .map_err(|e| format!("mcp: parse tool result: {e}"))?;
        Ok(tool_result)
    }

    /// 获取工具结果文本
    pub fn call_tool_text(&mut self, tool_name: &str, arguments: serde_json::Value) -> Result<String, String> {
        let result = self.call_tool(tool_name, arguments)?;
        let texts: Vec<String> = result.content.iter()
            .filter_map(|c| {
                if let McpContent::Text { text } = c { Some(text.clone()) } else { None }
            })
            .collect();
        if texts.is_empty() && !result.is_error {
            return Ok(String::new());
        }
        Ok(texts.join("\n"))
    }

    // ── 资源端点 ─────────────────────────────

    /// 发现 MCP 服务器上的资源
    pub fn discover_resources(&mut self) -> Result<Vec<McpResourceDef>, String> {
        let result = self.send_request("resources/list", None)?;
        let resources: Vec<McpResourceDef> = result
            .get("resources")
            .and_then(|v| serde_json::from_value::<Vec<McpResourceDef>>(v.clone()).ok())
            .unwrap_or_default();
        self.cached_resources = resources.clone();
        tracing::info!("mcp: discovered {} resources from '{}'", resources.len(), self.config.name);
        Ok(resources)
    }

    /// 读取资源内容
    pub fn read_resource(&mut self, uri: &str) -> Result<Vec<McpResourceContents>, String> {
        let params = serde_json::json!({ "uri": uri });
        let result = self.send_request("resources/read", Some(params))?;
        let contents: Vec<McpResourceContents> = result
            .get("contents")
            .and_then(|v| serde_json::from_value::<Vec<McpResourceContents>>(v.clone()).ok())
            .unwrap_or_default();
        Ok(contents)
    }

    // ── 提示端点 ─────────────────────────────

    /// 发现 MCP 服务器上的提示
    pub fn discover_prompts(&mut self) -> Result<Vec<McpPromptDef>, String> {
        let result = self.send_request("prompts/list", None)?;
        let prompts: Vec<McpPromptDef> = result
            .get("prompts")
            .and_then(|v| serde_json::from_value::<Vec<McpPromptDef>>(v.clone()).ok())
            .unwrap_or_default();
        self.cached_prompts = prompts.clone();
        tracing::info!("mcp: discovered {} prompts from '{}'", prompts.len(), self.config.name);
        Ok(prompts)
    }

    /// 获取提示内容
    pub fn get_prompt(&mut self, name: &str, arguments: Option<HashMap<String, String>>) -> Result<McpToolResult, String> {
        let args: Option<serde_json::Value> = arguments.map(|a| serde_json::to_value(a).unwrap_or_default());
        let mut params = serde_json::json!({ "name": name });
        if let Some(a) = args {
            params["arguments"] = a;
        }
        let result = self.send_request("prompts/get", Some(params))?;
        let prompt_result: McpToolResult = serde_json::from_value(result)
            .map_err(|e| format!("mcp: parse prompt result: {e}"))?;
        Ok(prompt_result)
    }

    // ── 全量发现 ─────────────────────────────

    /// 一次性发现所有（工具+资源+提示）
    pub fn discover_all(&mut self) -> Result<(Vec<McpTool>, Vec<McpResourceDef>, Vec<McpPromptDef>), Vec<String>> {
        let mut errors = Vec::new();
        let tools = self.discover_tools().unwrap_or_else(|e| { errors.push(e); Vec::new() });
        let resources = self.discover_resources().unwrap_or_else(|e| { errors.push(e); Vec::new() });
        let prompts = self.discover_prompts().unwrap_or_else(|e| { errors.push(e); Vec::new() });

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok((tools, resources, prompts))
    }

    // ── 访问器 ─────────────────────────────

    pub fn cached_tools(&self) -> &[McpTool] { &self.cached_tools }
    pub fn cached_resources(&self) -> &[McpResourceDef] { &self.cached_resources }
    pub fn cached_prompts(&self) -> &[McpPromptDef] { &self.cached_prompts }
    pub fn name(&self) -> &str { &self.config.name }
    pub fn prefix(&self) -> &str { &self.config.prefix }
}

// ─── 传输配置辅助方法 ────────────────────────

impl TransportConfig {
    /// 获取传输的 URL（仅 HTTP/SSE 模式有）
    pub fn url(&self) -> String {
        match self {
            TransportConfig::Http { url } | TransportConfig::Sse { url } => url.clone(),
            TransportConfig::Stdio { command, .. } => format!("stdio:{}", command),
        }
    }
}

// ─── MCP 管理器 ────────────────────────────────

/// 管理多个 MCP 客户端
pub struct McpManager {
    clients: Vec<McpClient>,
}

impl McpManager {
    pub fn new() -> Self {
        Self { clients: Vec::new() }
    }

    pub fn from_config(configs: Vec<McpServerConfig>) -> Self {
        let mut mgr = Self::new();
        for config in configs {
            mgr.add(config);
        }
        mgr
    }

    pub fn add(&mut self, config: McpServerConfig) {
        let mut client = McpClient::new(config);

        // Stdio 模式需要先 start 子进程
        if let TransportConfig::Stdio { .. } = &client.config.transport {
            if let Err(e) = client.spawn_stdio() {
                tracing::error!("mcp: failed to spawn stdio for '{}': {e}", client.name());
                return;
            }
        }

        self.clients.push(client);
    }

    /// 发现所有 MCP 服务的工具（带前缀去重）
    pub fn discover_all(&mut self) -> Vec<(String, String, Vec<McpTool>, Vec<McpResourceDef>, Vec<McpPromptDef>)> {
        let mut results = Vec::new();
        for client in &mut self.clients {
            let name = client.name().to_string();
            let prefix = client.prefix().to_string();
            let (tools, resources, prompts) = match client.discover_all() {
                Ok(t) => t,
                Err(errs) => {
                    for e in &errs {
                        tracing::warn!("mcp: failed to discover from '{}': {e}", name);
                    }
                    (Vec::new(), Vec::new(), Vec::new())
                }
            };
            results.push((name, prefix, tools, resources, prompts));
        }
        results
    }

    /// 根据工具名找到对应的客户端并调用
    pub fn call(&mut self, tool_name: &str, arguments: serde_json::Value) -> Result<String, String> {
        for client in &mut self.clients {
            let cached = client.cached_tools().to_vec();
            for tool in &cached {
                let mut names = vec![tool.name.clone()];
                if !client.prefix().is_empty() {
                    names.push(format!("{}.{}", client.prefix(), tool.name));
                }
                if names.contains(&tool_name.to_string()) {
                    return client.call_tool_text(&tool.name, arguments);
                }
            }
        }
        Err(format!("mcp: tool '{tool_name}' not found on any connected server"))
    }

    /// 获取所有已缓存的工具（自动加前缀去重）
    pub fn all_tools(&self) -> Vec<(String, String, serde_json::Value)> {
        let mut seen = std::collections::HashSet::new();
        let mut tools = Vec::new();
        for client in &self.clients {
            for tool in client.cached_tools() {
                let name = if client.prefix().is_empty() && !seen.contains(&tool.name) {
                    tool.name.clone()
                } else {
                    format!("{}.{}", client.prefix(), tool.name)
                };
                seen.insert(name.clone());
                tools.push((
                    name,
                    tool.description.clone(),
                    tool.input_schema.clone(),
                ));
            }
        }
        tools
    }

    /// 获取所有已缓存的资源
    pub fn all_resources(&self) -> Vec<(String, String, String)> {
        let mut resources = Vec::new();
        for client in &self.clients {
            for r in client.cached_resources() {
                resources.push((
                    format!("{}.{}", client.prefix(), r.name),
                    r.uri.clone(),
                    r.description.clone(),
                ));
            }
        }
        resources
    }

    /// 获取所有已缓存的提示
    pub fn all_prompts(&self) -> Vec<(String, String)> {
        let mut prompts = Vec::new();
        for client in &self.clients {
            for p in client.cached_prompts() {
                let name = if client.prefix().is_empty() {
                    p.name.clone()
                } else {
                    format!("{}.{}", client.prefix(), p.name)
                };
                prompts.push((name, p.description.clone()));
            }
        }
        prompts
    }

    pub fn client_count(&self) -> usize { self.clients.len() }
}

// ─── 测试 ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_format() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "tools/list".into(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""jsonrpc":"2.0""#));
        assert!(json.contains(r#""method":"tools/list""#));
        assert!(!json.contains(r#""params""#));
    }

    #[test]
    fn test_parse_mcp_tool_result() {
        let json = r#"{
            "content": [
                {"type": "text", "text": "Hello, world!"},
                {"type": "text", "text": "Line 2"}
            ],
            "is_error": false
        }"#;
        let result: McpToolResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.content.len(), 2);
        assert!(!result.is_error);
        if let McpContent::Text { text } = &result.content[0] {
            assert_eq!(text, "Hello, world!");
        } else {
            panic!("expected text content");
        }
    }

    #[test]
    fn test_parse_mcp_tool() {
        let json = r#"{
            "name": "calculator",
            "description": "A simple calculator",
            "input_schema": {
                "type": "object",
                "properties": {
                    "a": {"type": "number"},
                    "b": {"type": "number"}
                }
            }
        }"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "calculator");
    }

    #[test]
    fn test_transport_config_serialize() {
        // HTTP config
        let http = TransportConfig::Http { url: "http://localhost:8080".into() };
        let json = serde_json::to_string(&http).unwrap();
        assert!(json.contains(r#""transport":"http""#));

        // Stdio config
        let stdio = TransportConfig::Stdio {
            command: "node".into(),
            args: vec!["server.js".into()],
        };
        let json = serde_json::to_string(&stdio).unwrap();
        assert!(json.contains(r#""transport":"stdio""#));
        assert!(json.contains("server.js"));
    }

    #[test]
    fn test_parse_mcp_resource_def() {
        let json = r#"{
            "uri": "file:///tmp/test.txt",
            "name": "Test File",
            "description": "A test file",
            "mime_type": "text/plain"
        }"#;
        let res: McpResourceDef = serde_json::from_str(json).unwrap();
        assert_eq!(res.uri, "file:///tmp/test.txt");
        assert_eq!(res.name, "Test File");
    }

    #[test]
    fn test_parse_mcp_prompt_def() {
        let json = r#"{
            "name": "review",
            "description": "Code review prompt",
            "arguments": [
                {"name": "code", "description": "Code to review", "required": true}
            ]
        }"#;
        let prompt: McpPromptDef = serde_json::from_str(json).unwrap();
        assert_eq!(prompt.name, "review");
        assert_eq!(prompt.arguments.len(), 1);
    }
}
