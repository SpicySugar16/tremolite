use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// GitHub 搜索——gh search 的封装
pub struct GhSearchTool;

impl Tool for GhSearchTool {
    fn name(&self) -> &str { "gh_search" }
    fn description(&self) -> &str { "搜索 GitHub——仓库/代码/Issue/PR/用户。需要 gh CLI。" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "description": "搜索类型：repos / code / issues / prs / commits",
                    "enum": ["repos", "code", "issues", "prs", "commits"],
                    "default": "repos"
                },
                "query": { "type": "string", "description": "搜索关键词" },
                "limit": { "type": "integer", "description": "最大返回条数（默认 5）" },
                "owner": { "type": "string", "description": "限定仓库所有者（可选）" },
                "language": { "type": "string", "description": "限定编程语言（可选）" }
            },
            "required": ["query"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let query = match args.get("query") {
            Some(q) => q,
            None => return ToolResult {
                tool_name: "gh_search".into(), output: "缺少参数 query".into(), success: false,
            },
        };

        let search_type = args.get("type").map(|s| s.as_str()).unwrap_or("repos");
        let limit = args.get("limit").and_then(|s| s.parse::<u32>().ok()).unwrap_or(5);

        // 构建搜索查询
        let mut full_query = query.clone();
        if let Some(owner) = args.get("owner") {
            full_query = format!("{} org:{}", full_query, owner);
        }
        if let Some(lang) = args.get("language") {
            full_query = format!("{} language:{}", full_query, lang);
        }

        let mut cmd = Command::new("gh");
        cmd.args(&["search", search_type, &full_query, "--limit", &limit.to_string()]);

        match cmd.output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                ToolResult {
                    tool_name: "gh_search".into(),
                    output: if combined.is_empty() { "无搜索结果".into() } else { combined },
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "gh_search".into(),
                output: format!("gh 调用失败: {}（需要安装 gh CLI 并认证）", e),
                success: false,
            },
        }
    }
}
