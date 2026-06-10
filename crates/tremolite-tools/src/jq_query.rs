use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// JSON 数据查询（jq 风格）
pub struct JqQueryTool;

impl Tool for JqQueryTool {
    fn name(&self) -> &str { "jq_query" }
    fn description(&self) -> &str { "对 JSON 数据执行 jq 查询（需要安装 jq）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "jq 查询表达式，如 '.key' '.items[] | .name'"},
                "data": {"type": "string", "description": "JSON 字符串（直接输入数据）"},
                "file": {"type": "string", "description": "JSON 文件路径（与 data 二选一）"},
                "raw": {"type": "boolean", "description": "是否输出原始字符串（-r 参数）"}
            },
            "required": ["query"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let query = match args.get("query") {
            Some(q) => q,
            None => return ToolResult {
                tool_name: "jq_query".into(),
                output: "缺少参数 query".into(),
                success: false,
            },
        };
        let raw = args.get("raw").map(|s| s == "true").unwrap_or(false);

        let mut cmd = Command::new("jq");
        if raw {
            cmd.arg("-r");
        }
        cmd.arg(query);

        if let Some(file) = args.get("file") {
            cmd.arg(file);
        } else if let Some(data) = args.get("data") {
            cmd.arg(data);
        } else {
            // 从 stdin 读空 JSON
            cmd.arg(".");
        }

        let output = cmd.output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                let truncated = if combined.len() > 10000 {
                    format!("{}...\n[截断至 10000 字符]", &combined[..10000])
                } else {
                    combined
                };
                ToolResult {
                    tool_name: "jq_query".into(),
                    output: truncated,
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "jq_query".into(),
                output: format!("jq 调用失败: {}（需要安装 jq）", e),
                success: false,
            },
        }
    }
}
