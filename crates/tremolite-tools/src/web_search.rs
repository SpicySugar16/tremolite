use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// 网络搜索——通过本地 SearXNG 实例搜索互联网
pub struct WebSearchTool;

impl Tool for WebSearchTool {
    fn name(&self) -> &str { "web_search" }
    fn description(&self) -> &str { "互联网搜索——通过本地 SearXNG 搜索引擎搜索网页内容" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "limit": {
                    "type": "integer",
                    "description": "最大返回条数（默认 5）",
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let query = match args.get("query") {
            Some(q) => q,
            None => return ToolResult {
                tool_name: "web_search".into(),
                output: "缺少参数 query".into(),
                success: false,
            },
        };

        let limit = args.get("limit")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(5);

        // 调用本地 SearXNG 搜索脚本
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/spicysugar".into());
        let script_path = format!("{}/searxng/search.sh", home);

        let mut cmd = Command::new("bash");
        cmd.arg(&script_path);
        cmd.arg(query);
        cmd.arg(&limit.to_string());

        // https_proxy 可能会干扰本地 SearXNG，清理掉
        cmd.env_remove("https_proxy");
        cmd.env_remove("HTTPS_PROXY");
        cmd.env_remove("http_proxy");
        cmd.env_remove("HTTP_PROXY");

        match cmd.output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();

                if out.status.success() {
                    ToolResult {
                        tool_name: "web_search".into(),
                        output: if stdout.is_empty() { "无搜索结果".into() } else { stdout },
                        success: true,
                    }
                } else {
                    let combined = if stderr.is_empty() { stdout } else { format!("{}\n(stderr) {}", stdout, stderr) };
                    ToolResult {
                        tool_name: "web_search".into(),
                        output: format!("搜索失败: {}", combined),
                        success: false,
                    }
                }
            }
            Err(e) => ToolResult {
                tool_name: "web_search".into(),
                output: format!("搜索脚本执行失败: {}（需要安装 SearXNG）", e),
                success: false,
            },
        }
    }
}
