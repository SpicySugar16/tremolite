use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct WriteFileTool;

impl Tool for WriteFileTool {
    fn name(&self) -> &str { "write_file" }
    fn description(&self) -> &str { "写入文件内容" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "文件路径"},
                "content": {"type": "string", "description": "要写入的内容"}
            },
            "required": ["path"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = match args.get("path") {
            Some(p) => p,
            None => return ToolResult {
                tool_name: "write_file".into(),
                output: "Error: missing 'path' argument".into(),
                success: false,
            },
        };
        let content = args.get("content").map(|s| s.as_str()).unwrap_or("");

        match fs::write(Path::new(path), content) {
            Ok(()) => ToolResult {
                tool_name: "write_file".into(),
                output: format!("Written {} bytes to {}", content.len(), path),
                success: true,
            },
            Err(e) => ToolResult {
                tool_name: "write_file".into(),
                output: format!("Error: {}", e),
                success: false,
            },
        }
    }
}
