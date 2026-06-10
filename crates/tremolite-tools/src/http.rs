use crate::{Tool, ToolResult};
use std::collections::HashMap;

pub struct HttpTool;

impl Tool for HttpTool {
    fn name(&self) -> &str { "http" }
    fn description(&self) -> &str { "发送 HTTP 请求" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "请求 URL"},
                "method": {"type": "string", "description": "HTTP 方法 (GET/POST/PUT/DELETE)", "default": "GET"},
                "body": {"type": "string", "description": "请求体（POST/PUT 时需要）"}
            },
            "required": ["url"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let url = match args.get("url") {
            Some(u) => u,
            None => return ToolResult {
                tool_name: "http".into(),
                output: "Error: missing 'url' argument".into(),
                success: false,
            },
        };

        let method = args.get("method").map(|s| s.as_str()).unwrap_or("GET");

        // ureq v2.12: request() returns Request directly
        let req = ureq::request(method, url);
        match req.call() {
            Ok(resp) => {
                let status = resp.status();
                let body = resp.into_string().unwrap_or_default();
                let truncated = if body.len() > 3000 {
                    format!("{}...\n[truncated at 3000 chars]", &body[..3000])
                } else {
                    body
                };
                ToolResult {
                    tool_name: "http".into(),
                    output: format!("HTTP {}:\n{}", status, truncated),
                    success: status < 500,
                }
            }
            Err(e) => ToolResult {
                tool_name: "http".into(),
                output: format!("Request failed: {}", e),
                success: false,
            },
        }
    }
}
