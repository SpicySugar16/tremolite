use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

pub struct GitStatusTool;

impl Tool for GitStatusTool {
    fn name(&self) -> &str { "git_status" }
    fn description(&self) -> &str { "显示 Git 仓库状态（git status 输出）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Git 仓库路径（默认当前目录）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
        run_git(&["status", "--short"], &path)
    }
}

pub struct GitDiffTool;

impl Tool for GitDiffTool {
    fn name(&self) -> &str { "git_diff" }
    fn description(&self) -> &str { "查看 Git diff（暂存区 vs 工作区）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Git 仓库路径（默认当前目录）"},
                "staged": {"type": "boolean", "description": "是否查看已暂存的 diff（--cached）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
        let staged = args.get("staged").map(|s| s == "true").unwrap_or(false);
        if staged {
            run_git(&["diff", "--cached"], &path)
        } else {
            run_git(&["diff"], &path)
        }
    }
}

pub struct GitLogTool;

impl Tool for GitLogTool {
    fn name(&self) -> &str { "git_log" }
    fn description(&self) -> &str { "查看 Git 提交历史" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Git 仓库路径（默认当前目录）"},
                "count": {"type": "integer", "description": "显示条数（默认 10）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
        let count = args.get("count").and_then(|s| s.parse::<usize>().ok()).unwrap_or(10);
        run_git(&["log", "--oneline", &format!("-{}", count), "--"], &path)
    }
}

pub struct GitCommitTool;

impl Tool for GitCommitTool {
    fn name(&self) -> &str { "git_commit" }
    fn description(&self) -> &str { "创建 Git 提交（先 add 再 commit）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "提交信息"},
                "path": {"type": "string", "description": "Git 仓库路径（默认当前目录）"},
                "all": {"type": "boolean", "description": "是否自动暂存所有变更（git add -A）"}
            },
            "required": ["message"]
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
        let all = args.get("all").map(|s| s == "true").unwrap_or(true);
        let msg = args.get("message").unwrap();

        if all {
            let add = run_git(&["add", "-A"], &path);
            if !add.success {
                return add;
            }
        }
        run_git(&["commit", "-m", msg], &path)
    }
}

pub struct GitPushTool;

impl Tool for GitPushTool {
    fn name(&self) -> &str { "git_push" }
    fn description(&self) -> &str { "推送 Git 提交到远程仓库" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "remote": {"type": "string", "description": "远程名称（默认 origin）"},
                "branch": {"type": "string", "description": "分支名"},
                "path": {"type": "string", "description": "Git 仓库路径（默认当前目录）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned().unwrap_or_else(|| ".".into());
        let remote = args.get("remote").cloned().unwrap_or_else(|| "origin".into());
        let branch = args.get("branch").cloned();
        let mut cmd = vec!["push", &remote];
        if let Some(ref b) = branch {
            cmd.push(b);
        }
        run_git(&cmd, &path)
    }
}

fn run_git(args: &[&str], path: &str) -> ToolResult {
    match Command::new("git").args(args).current_dir(path).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
            ToolResult {
                tool_name: format!("git_{}", args[0]),
                output: if combined.is_empty() { "(空结果)".into() } else { combined },
                success: output.status.success(),
            }
        }
        Err(e) => ToolResult {
            tool_name: format!("git_{}", args[0]),
            output: format!("Git 调用失败: {}", e),
            success: false,
        },
    }
}
