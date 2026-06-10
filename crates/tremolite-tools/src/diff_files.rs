use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// 比较两个文件的差异（diff -u）
pub struct DiffFilesTool;

impl Tool for DiffFilesTool {
    fn name(&self) -> &str { "diff_files" }
    fn description(&self) -> &str { "比较两个文件的差异（diff -u）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file1": {"type": "string", "description": "第一个文件路径"},
                "file2": {"type": "string", "description": "第二个文件路径"},
                "context": {"type": "integer", "description": "上下文行数（默认 3）"}
            },
            "required": ["file1", "file2"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let file1 = match args.get("file1") {
            Some(f) => f,
            None => return ToolResult {
                tool_name: "diff_files".into(),
                output: "缺少参数 file1".into(),
                success: false,
            },
        };
        let file2 = match args.get("file2") {
            Some(f) => f,
            None => return ToolResult {
                tool_name: "diff_files".into(),
                output: "缺少参数 file2".into(),
                success: false,
            },
        };
        let ctx = args.get("context").and_then(|s| s.parse::<usize>().ok()).unwrap_or(3);

        let output = Command::new("diff")
            .arg("-U")
            .arg(ctx.to_string())
            .arg(file1)
            .arg(file2)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                let result = if combined.is_empty() {
                    "两个文件内容完全相同".into()
                } else {
                    let truncated = if combined.len() > 10000 {
                        format!("{}...\n[截断至 10000 字符]", &combined[..10000])
                    } else {
                        combined
                    };
                    truncated
                };
                ToolResult {
                    tool_name: "diff_files".into(),
                    output: result,
                    success: out.status.success() || out.status.code() == Some(1),
                }
            }
            Err(e) => ToolResult {
                tool_name: "diff_files".into(),
                output: format!("diff 调用失败: {}（需要安装 diffutils）", e),
                success: false,
            },
        }
    }
}
