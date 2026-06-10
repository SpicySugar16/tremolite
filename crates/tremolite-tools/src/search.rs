use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct SearchTool;

impl Tool for SearchTool {
    fn name(&self) -> &str { "search" }
    fn description(&self) -> &str { "搜索文件内容" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "搜索关键词或正则表达式"},
                "path": {"type": "string", "description": "搜索路径", "default": "."},
                "limit": {"type": "integer", "description": "最大结果数", "default": 20}
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let pattern = match args.get("pattern") {
            Some(p) => p,
            None => return ToolResult {
                tool_name: "search".into(),
                output: "Error: missing 'pattern' argument".into(),
                success: false,
            },
        };

        let path = args.get("path").map(|s| s.as_str()).unwrap_or(".");
        let max_results = args.get("limit")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(20);

        let mut results = Vec::new();
        let lower_pattern = pattern.to_lowercase();

        if let Ok(entries) = fs::read_dir(Path::new(path)) {
            for entry in entries.flatten() {
                if results.len() >= max_results {
                    break;
                }
                let entry_path = entry.path();
                if entry_path.is_file() {
                    if let Ok(content) = fs::read_to_string(&entry_path) {
                        for (i, line) in content.lines().enumerate() {
                            if line.to_lowercase().contains(&lower_pattern) {
                                results.push(format!("{}:{}: {}",
                                    entry_path.display(), i + 1,
                                    line.trim().chars().take(100).collect::<String>()));
                                if results.len() >= max_results {
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        if results.is_empty() {
            ToolResult {
                tool_name: "search".into(),
                output: format!("No matches for '{}'", pattern),
                success: true,
            }
        } else {
            ToolResult {
                tool_name: "search".into(),
                output: results.join("\n"),
                success: true,
            }
        }
    }
}
