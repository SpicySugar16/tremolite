use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// 网络连通性测试（ping）
pub struct PingTool;

impl Tool for PingTool {
    fn name(&self) -> &str { "ping" }
    fn description(&self) -> &str { "测试主机的网络连通性（ping）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "host": {"type": "string", "description": "目标主机名或 IP 地址"},
                "count": {"type": "integer", "description": "发送次数（默认 4）"},
                "timeout": {"type": "integer", "description": "超时秒数（默认 10）"}
            },
            "required": ["host"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let host = match args.get("host") {
            Some(h) => h,
            None => return ToolResult {
                tool_name: "ping".into(),
                output: "缺少参数 host".into(),
                success: false,
            },
        };
        let count = args.get("count").and_then(|s| s.parse::<u32>().ok()).unwrap_or(4);
        let timeout = args.get("timeout").and_then(|s| s.parse::<u32>().ok()).unwrap_or(10);

        let output = Command::new("ping")
            .arg("-c")
            .arg(count.to_string())
            .arg("-W")
            .arg(timeout.to_string())
            .arg(host)
            .output();

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                let truncated = if combined.len() > 5000 {
                    format!("{}...\n[截断至 5000 字符]", &combined[..5000])
                } else {
                    combined
                };
                ToolResult {
                    tool_name: "ping".into(),
                    output: truncated,
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "ping".into(),
                output: format!("ping 调用失败: {}", e),
                success: false,
            },
        }
    }
}
