use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;

pub struct RmFileTool;

impl Tool for RmFileTool {
    fn name(&self) -> &str { "rm_file" }
    fn description(&self) -> &str { "删除文件或目录" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "要删除的路径"},
                "recursive": {"type": "boolean", "description": "是否递归删除目录"}
            },
            "required": ["path"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").unwrap();
        let recursive = args.get("recursive").map(|s| s == "true").unwrap_or(false);

        let result = if recursive && fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false) {
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        } else {
            fs::remove_file(path).or_else(|_| fs::remove_dir(path)).map_err(|e| e.to_string())
        };

        match result {
            Ok(()) => ToolResult { tool_name: "rm_file".into(), output: format!("已删除: {}", path), success: true },
            Err(e) => ToolResult { tool_name: "rm_file".into(), output: format!("删除失败: {}", e), success: false },
        }
    }
}
