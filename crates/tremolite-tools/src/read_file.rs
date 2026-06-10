use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct ReadFileTool;

impl Tool for ReadFileTool {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "读取文件内容" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "文件路径"}
            },
            "required": ["path"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = match args.get("path") {
            Some(p) => p,
            None => return ToolResult {
                tool_name: "read_file".into(),
                output: "Error: missing 'path' argument".into(),
                success: false,
            },
        };

        match fs::read_to_string(Path::new(path)) {
            Ok(content) => {
                let truncated = if content.len() > 10000 {
                    format!("{}...\n[truncated at 10000 chars]", &content[..10000])
                } else {
                    content
                };
                ToolResult { tool_name: "read_file".into(), output: truncated, success: true }
            }
            Err(e) => ToolResult {
                tool_name: "read_file".into(),
                output: format!("Error: {}", e),
                success: false,
            },
        }
    }
}
