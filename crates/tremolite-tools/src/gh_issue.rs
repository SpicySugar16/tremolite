use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// GitHub Issue 管理——gh issue 的封装
pub struct GhIssueTool;

impl Tool for GhIssueTool {
    fn name(&self) -> &str { "gh_issue" }
    fn description(&self) -> &str { "GitHub Issue 管理——创建/列出/查看/关闭 Issue。需要 gh CLI 和 GitHub 认证。" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "操作类型：list（列表）/ view（查看）/ create（创建）/ close（关闭）",
                    "enum": ["list", "view", "create", "close"]
                },
                "repo": {
                    "type": "string",
                    "description": "仓库名，格式 owner/repo。省略则使用当前目录的 Git 仓库"
                },
                "number": {
                    "type": "integer",
                    "description": "Issue 编号，用于 view/close 操作"
                },
                "title": {
                    "type": "string",
                    "description": "Issue 标题，用于 create 操作"
                },
                "body": {
                    "type": "string",
                    "description": "Issue 正文，用于 create 操作"
                },
                "limit": {
                    "type": "integer",
                    "description": "最大返回条数（默认 10）"
                },
                "state": {
                    "type": "string",
                    "description": "筛选状态：open / closed / all（默认 open，仅 list 有效）",
                    "enum": ["open", "closed", "all"]
                }
            },
            "required": ["action"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let action = match args.get("action") {
            Some(a) => a.as_str(),
            None => return ToolResult {
                tool_name: "gh_issue".into(),
                output: "缺少参数 action".into(),
                success: false,
            },
        };

        match action {
            "list" => self.list_issues(args),
            "view" => self.view_issue(args),
            "create" => self.create_issue(args),
            "close" => self.close_issue(args),
            _ => ToolResult {
                tool_name: "gh_issue".into(),
                output: format!("未知 action '{}'，可选: list / view / create / close", action),
                success: false,
            },
        }
    }
}

impl GhIssueTool {
    fn repo_str(&self, args: &HashMap<String, String>) -> Option<String> {
        args.get("repo").cloned()
    }

    fn exec(&self, cmd: &mut Command) -> ToolResult {
        match cmd.output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                ToolResult {
                    tool_name: "gh_issue".into(),
                    output: if combined.is_empty() { "操作成功（无输出）".into() } else { combined },
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "gh_issue".into(),
                output: format!("gh 调用失败: {}（需要安装 gh CLI 并认证）", e),
                success: false,
            },
        }
    }

    fn list_issues(&self, args: &HashMap<String, String>) -> ToolResult {
        let repo = self.repo_str(args);
        let state = args.get("state").map(|s| s.as_str()).unwrap_or("open");
        let limit = args.get("limit").and_then(|s| s.parse::<u32>().ok()).unwrap_or(10);
        let limit_str = limit.to_string();

        let gh_args = vec!["issue", "list", "--state", state, "--limit", &limit_str];

        let mut cmd = Command::new("gh");
        cmd.args(&gh_args);
        if let Some(ref r) = repo {
            cmd.arg("-R");
            cmd.arg(r);
        }
        self.exec(&mut cmd)
    }

    fn view_issue(&self, args: &HashMap<String, String>) -> ToolResult {
        let number = match args.get("number").and_then(|s| s.parse::<u32>().ok()) {
            Some(n) => n,
            None => return ToolResult {
                tool_name: "gh_issue".into(),
                output: "缺少有效 number 参数".into(),
                success: false,
            },
        };

        let repo = self.repo_str(args);
        let mut cmd = Command::new("gh");
        cmd.args(&["issue", "view", &number.to_string()]);
        if let Some(ref r) = repo {
            cmd.arg("-R");
            cmd.arg(r);
        }
        self.exec(&mut cmd)
    }

    fn create_issue(&self, args: &HashMap<String, String>) -> ToolResult {
        let title = match args.get("title") {
            Some(t) => t,
            None => return ToolResult {
                tool_name: "gh_issue".into(),
                output: "缺少 title 参数".into(),
                success: false,
            },
        };

        let repo = self.repo_str(args);
        let body = args.get("body").map(|s| s.as_str()).unwrap_or("");

        let mut cmd = Command::new("gh");
        cmd.args(&["issue", "create", "--title", title]);
        if let Some(ref r) = repo {
            cmd.arg("-R");
            cmd.arg(r);
        }
        if !body.is_empty() {
            cmd.arg("--body");
            cmd.arg(body);
        }
        self.exec(&mut cmd)
    }

    fn close_issue(&self, args: &HashMap<String, String>) -> ToolResult {
        let number = match args.get("number").and_then(|s| s.parse::<u32>().ok()) {
            Some(n) => n,
            None => return ToolResult {
                tool_name: "gh_issue".into(),
                output: "缺少有效 number 参数".into(),
                success: false,
            },
        };

        let repo = self.repo_str(args);
        let mut cmd = Command::new("gh");
        cmd.args(&["issue", "close", &number.to_string()]);
        if let Some(ref r) = repo {
            cmd.arg("-R");
            cmd.arg(r);
        }
        self.exec(&mut cmd)
    }
}
