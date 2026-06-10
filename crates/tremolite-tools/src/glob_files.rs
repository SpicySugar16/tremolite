use crate::{Tool, ToolResult};
use std::collections::HashMap;

pub struct GlobFilesTool;

impl Tool for GlobFilesTool {
    fn name(&self) -> &str { "glob_files" }
    fn description(&self) -> &str { "按通配符模式搜索文件（如 *.rs 或 **/*.toml）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "glob 通配符模式"},
                "base": {"type": "string", "description": "搜索基目录（默认当前目录）"}
            },
            "required": ["pattern"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let pattern = args.get("pattern").unwrap();
        let base = args.get("base").cloned().unwrap_or_else(|| ".".into());

        let full_pattern = if pattern.starts_with('/') || pattern.starts_with("~") {
            pattern.clone()
        } else {
            format!("{}/{}", base.trim_end_matches('/'), pattern)
        };

        match glob::glob(&full_pattern) {
            Ok(paths) => {
                let items: Vec<String> = paths
                    .filter_map(|p| p.ok())
                    .map(|p| p.to_string_lossy().to_string())
                    .collect();
                let output = if items.is_empty() {
                    format!("没有匹配: {}", full_pattern)
                } else {
                    format!("找到 {} 个匹配:\n{}", items.len(), items.join("\n"))
                };
                ToolResult { tool_name: "glob_files".into(), output, success: true }
            }
            Err(e) => ToolResult { tool_name: "glob_files".into(), output: format!("搜索失败: {}", e), success: false },
        }
    }
}
