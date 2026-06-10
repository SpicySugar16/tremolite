use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// DNS 查询（nslookup / dig）
pub struct DnsLookupTool;

impl Tool for DnsLookupTool {
    fn name(&self) -> &str { "dns_lookup" }
    fn description(&self) -> &str { "DNS 解析查询——查询域名的 A/AAAA/MX 记录" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "hostname": {"type": "string", "description": "要查询的域名，如 example.com"},
                "type": {"type": "string", "description": "记录类型：A / AAAA / MX / TXT / CNAME（默认 A）"},
                "dns_server": {"type": "string", "description": "指定 DNS 服务器（可选）"}
            },
            "required": ["hostname"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let hostname = match args.get("hostname") {
            Some(h) => h,
            None => return ToolResult {
                tool_name: "dns_lookup".into(),
                output: "缺少参数 hostname".into(),
                success: false,
            },
        };
        let record_type = args.get("type").map(|s| s.as_str()).unwrap_or("A");
        let dns_server = args.get("dns_server");

        // 优先用 dig（更详细的输出），fallback 到 nslookup
        let mut cmd = Command::new("dig");
        cmd.arg(hostname)
            .arg("-t")
            .arg(record_type)
            .arg("+short");
        if let Some(dns) = dns_server {
            cmd.arg(format!("@{}", dns));
        }

        let output = cmd.output();

        // dig 不存在 → 用 nslookup
        if output.as_ref().err().map(|e| e.kind() == std::io::ErrorKind::NotFound).unwrap_or(false) {
            let mut ns = Command::new("nslookup");
            ns.arg(hostname);
            if let Some(dns) = dns_server {
                let dns_addr = dns.to_string();
                ns.arg(&dns_addr);
            }
            let ns_result = ns.output();
            match ns_result {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    return ToolResult {
                        tool_name: "dns_lookup".into(),
                        output: stdout,
                        success: out.status.success(),
                    };
                }
                Err(e) => return ToolResult {
                    tool_name: "dns_lookup".into(),
                    output: format!("DNS 查询失败: {}（需要安装 dig 或 nslookup）", e),
                    success: false,
                },
            }
        }

        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                let result = if combined.is_empty() {
                    format!("{} 没有 {} 记录", hostname, record_type)
                } else {
                    combined
                };
                ToolResult {
                    tool_name: "dns_lookup".into(),
                    output: result,
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "dns_lookup".into(),
                output: format!("dig 调用失败: {}", e),
                success: false,
            },
        }
    }
}
