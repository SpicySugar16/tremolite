use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

/// GitHub PR 管理——gh pr 的封装
pub struct GhPrTool;

impl Tool for GhPrTool {
    fn name(&self) -> &str { "gh_pr" }
    fn description(&self) -> &str { "GitHub Pull Request 管理——列出/查看/创建/合并 PR" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "description": "操作类型：list / view / create / merge",
                    "enum": ["list", "view", "create", "merge"]
                },
                "repo": { "type": "string", "description": "仓库名，格式 owner/repo（可选，默认当前目录）" },
                "number": { "type": "integer", "description": "PR 编号，用于 view/merge" },
                "title": { "type": "string", "description": "PR 标题，用于 create" },
                "body": { "type": "string", "description": "PR 正文，用于 create" },
                "head": { "type": "string", "description": "源分支，用于 create" },
                "base": { "type": "string", "description": "目标分支，用于 create（默认 main）" },
                "limit": { "type": "integer", "description": "最大返回条数（默认 10）" },
                "state": {
                    "type": "string", "description": "筛选状态：open / closed / merged / all（默认 open）",
                    "enum": ["open", "closed", "merged", "all"]
                }
            },
            "required": ["action"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let action = match args.get("action").map(|s| s.as_str()) {
            Some(a) => a,
            None => return ToolResult {
                tool_name: "gh_pr".into(), output: "缺少参数 action".into(), success: false,
            },
        };

        match action {
            "list" => self.list_prs(args),
            "view" => self.view_pr(args),
            "create" => self.create_pr(args),
            "merge" => self.merge_pr(args),
            _ => ToolResult {
                tool_name: "gh_pr".into(),
                output: format!("未知 action '{}'，可选: list / view / create / merge", action),
                success: false,
            },
        }
    }
}

impl GhPrTool {
    fn repo_ref(&self, args: &HashMap<String, String>) -> Option<String> {
        args.get("repo").cloned()
    }

    fn exec(&self, cmd: &mut Command) -> ToolResult {
        match cmd.output() {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
                ToolResult {
                    tool_name: "gh_pr".into(),
                    output: if combined.is_empty() { "操作成功（无输出）".into() } else { combined },
                    success: out.status.success(),
                }
            }
            Err(e) => ToolResult {
                tool_name: "gh_pr".into(),
                output: format!("gh 调用失败: {}（需要安装 gh CLI 并认证）", e),
                success: false,
            },
        }
    }

    fn build_cmd(&self, args: &[&str], repo: Option<&str>) -> Command {
        let mut cmd = Command::new("gh");
        cmd.args(args);
        if let Some(r) = repo {
            cmd.arg("-R");
            cmd.arg(r);
        }
        cmd
    }

    fn list_prs(&self, args: &HashMap<String, String>) -> ToolResult {
        let state = args.get("state").map(|s| s.as_str()).unwrap_or("open");
        let limit = args.get("limit").and_then(|s| s.parse::<u32>().ok()).unwrap_or(10);
        let repo = self.repo_ref(args);
        self.exec(&mut self.build_cmd(&["pr", "list", "--state", state, "--limit", &limit.to_string()], repo.as_deref()))
    }

    fn view_pr(&self, args: &HashMap<String, String>) -> ToolResult {
        let number = match args.get("number").and_then(|s| s.parse::<u32>().ok()) {
            Some(n) => n,
            None => return ToolResult {
                tool_name: "gh_pr".into(), output: "缺少有效 number 参数".into(), success: false,
            },
        };
        let repo = self.repo_ref(args);
        self.exec(&mut self.build_cmd(&["pr", "view", &number.to_string()], repo.as_deref()))
    }

    fn create_pr(&self, args: &HashMap<String, String>) -> ToolResult {
        let title = match args.get("title") {
            Some(t) => t,
            None => return ToolResult {
                tool_name: "gh_pr".into(), output: "缺少 title 参数".into(), success: false,
            },
        };
        let repo = self.repo_ref(args);
        let base = args.get("base").map(|s| s.as_str()).unwrap_or("main");

        let mut cmd = self.build_cmd(&["pr", "create", "--title", title, "--base", base], repo.as_deref());
        if let Some(body) = args.get("body") {
            cmd.arg("--body");
            cmd.arg(body);
        }
        if let Some(head) = args.get("head") {
            cmd.arg("--head");
            cmd.arg(head);
        }
        self.exec(&mut cmd)
    }

    fn merge_pr(&self, args: &HashMap<String, String>) -> ToolResult {
        let number = match args.get("number").and_then(|s| s.parse::<u32>().ok()) {
            Some(n) => n,
            None => return ToolResult {
                tool_name: "gh_pr".into(), output: "缺少有效 number 参数".into(), success: false,
            },
        };
        let repo = self.repo_ref(args);
        self.exec(&mut self.build_cmd(&["pr", "merge", &number.to_string(), "--merge"], repo.as_deref()))
    }
}
