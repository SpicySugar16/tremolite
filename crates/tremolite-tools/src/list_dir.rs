use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;

pub struct ListDirTool;

impl Tool for ListDirTool {
    fn name(&self) -> &str { "list_dir" }
    fn description(&self) -> &str { "列出目录下文件和子目录" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "目录路径（默认当前目录）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
        match fs::read_dir(&path) {
            Ok(entries) => {
                let mut items: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .map(|e| {
                        let name = e.file_name().to_string_lossy().to_string();
                        if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            format!("{}/", name)
                        } else {
                            name
                        }
                    })
                    .collect();
                items.sort();
                let output = if items.is_empty() {
                    format!("（空目录）: {}", path)
                } else {
                    format!("{} 中 {} 项:\n{}", path, items.len(), items.join("\n"))
                };
                ToolResult { tool_name: "list_dir".into(), output, success: true }
            }
            Err(e) => ToolResult { tool_name: "list_dir".into(), output: format!("读取目录失败: {}", e), success: false },
        }
    }
}
