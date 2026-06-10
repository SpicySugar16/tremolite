use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;
use std::time::Instant;

pub struct ShellTool;

impl Tool for ShellTool {
    fn name(&self) -> &str { "shell" }
    fn description(&self) -> &str { "执行 shell 命令" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "要执行的 shell 命令"},
                "timeout": {"type": "integer", "description": "超时秒数（可选）", "default": 10}
            },
            "required": ["command"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let cmd = match args.get("command") {
            Some(c) => c,
            None => return ToolResult {
                tool_name: "shell".into(),
                output: "Error: missing 'command' argument".into(),
                success: false,
            },
        };

        let timeout_ms = args.get("timeout")
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(10);

        let start = Instant::now();
        let output = Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output();

        let elapsed = start.elapsed().as_secs_f64();

        match output {
            Ok(out) => {
                let mut result = String::new();
                if !out.stdout.is_empty() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    result.push_str(&stdout);
                }
                if !out.stderr.is_empty() {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    if !result.is_empty() { result.push('\n'); }
                    result.push_str(&stderr);
                }
                if result.is_empty() {
                    result = format!("[exit code: {}]", out.status.code().unwrap_or(-1));
                }

                let truncated = if result.len() > 5000 {
                    format!("{}...\n[truncated at 5000 chars]", &result[..5000])
                } else {
                    result
                };

                ToolResult {
                    tool_name: "shell".into(),
                    output: format!("({:.2}s) {}", elapsed, truncated),
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "shell".into(),
                output: format!("Error: {}", e),
                success: false,
            },
        }
    }
}
