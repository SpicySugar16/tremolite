use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;

pub struct MvFileTool;

impl Tool for MvFileTool {
    fn name(&self) -> &str { "mv_file" }
    fn description(&self) -> &str { "移动或重命名文件/目录" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "src": {"type": "string", "description": "源路径"},
                "dst": {"type": "string", "description": "目标路径"}
            },
            "required": ["src", "dst"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let src = args.get("src").unwrap();
        let dst = args.get("dst").unwrap();
        match fs::rename(src, dst) {
            Ok(()) => ToolResult { tool_name: "mv_file".into(), output: format!("已移动: {} -> {}", src, dst), success: true },
            Err(e) => ToolResult { tool_name: "mv_file".into(), output: format!("移动失败: {}", e), success: false },
        }
    }
}
