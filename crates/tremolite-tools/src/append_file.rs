use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;

pub struct AppendFileTool;

impl Tool for AppendFileTool {
    fn name(&self) -> &str { "append_file" }
    fn description(&self) -> &str { "追加内容到文件末尾" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "文件路径"},
                "content": {"type": "string", "description": "要追加的内容"}
            },
            "required": ["path", "content"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").unwrap();
        let content = args.get("content").unwrap();
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                match writeln!(file, "{}", content) {
                    Ok(()) => ToolResult { tool_name: "append_file".into(), output: format!("已追加到: {}", path), success: true },
                    Err(e) => ToolResult { tool_name: "append_file".into(), output: format!("写入失败: {}", e), success: false },
                }
            }
            Err(e) => ToolResult { tool_name: "append_file".into(), output: format!("打开文件失败: {}", e), success: false },
        }
    }
}
