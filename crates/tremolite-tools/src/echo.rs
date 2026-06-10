use crate::{Tool, ToolResult};
use std::collections::HashMap;

pub struct EchoTool;

impl Tool for EchoTool {
    fn name(&self) -> &str { "echo" }
    fn description(&self) -> &str { "回显输入内容" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "要回显的文本"}
            }
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let output = args.get("text").cloned().unwrap_or_default();
        ToolResult { tool_name: "echo".into(), output, success: true }
    }
}
