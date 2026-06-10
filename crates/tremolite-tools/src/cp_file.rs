use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct CpFileTool;

impl Tool for CpFileTool {
    fn name(&self) -> &str { "cp_file" }
    fn description(&self) -> &str { "复制文件或目录" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "src": {"type": "string", "description": "源路径"},
                "dst": {"type": "string", "description": "目标路径"},
                "recursive": {"type": "boolean", "description": "是否递归复制目录"}
            },
            "required": ["src", "dst"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let src = args.get("src").unwrap();
        let dst = args.get("dst").unwrap();
        let recursive = args.get("recursive").map(|s| s == "true").unwrap_or(false);

        let result = if recursive {
            cp_recursive(Path::new(src), Path::new(dst))
        } else {
            fs::copy(src, dst).map(|_| ()).map_err(|e| e.to_string())
        };

        match result {
            Ok(()) => ToolResult { tool_name: "cp_file".into(), output: format!("已复制: {} -> {}", src, dst), success: true },
            Err(e) => ToolResult { tool_name: "cp_file".into(), output: format!("复制失败: {}", e), success: false },
        }
    }
}

fn cp_recursive(src: &Path, dst: &Path) -> Result<(), String> {
    if src.is_dir() {
        fs::create_dir_all(dst).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            cp_recursive(&child_src, &child_dst)?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ()).map_err(|e| e.to_string())
    }
}
