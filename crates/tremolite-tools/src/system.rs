use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::process::Command;

pub struct DiskUsageTool;

impl Tool for DiskUsageTool {
    fn name(&self) -> &str { "disk_usage" }
    fn description(&self) -> &str { "显示磁盘使用情况（df -h）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "查看指定路径（默认所有挂载点）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let path = args.get("path").cloned();
        let mut cmd = vec!["-h"];
        if let Some(ref p) = path {
            cmd.push(p);
        }
        run_sys("df", &cmd)
    }
}

pub struct MemoryInfoTool;

impl Tool for MemoryInfoTool {
    fn name(&self) -> &str { "memory_info" }
    fn description(&self) -> &str { "显示内存使用情况（free -h）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    fn execute(&self, _args: &HashMap<String, String>) -> ToolResult {
        run_sys("free", &["-h"])
    }
}

pub struct ProcessListTool;

impl Tool for ProcessListTool {
    fn name(&self) -> &str { "process_list" }
    fn description(&self) -> &str { "列出进程（ps aux）" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "filter": {"type": "string", "description": "按进程名过滤（grep 关键词）"},
                "count": {"type": "integer", "description": "显示条数（默认 20）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let count = args.get("count").and_then(|s| s.parse::<usize>().ok()).unwrap_or(20);
        let filter = args.get("filter").cloned();

        let output = run_sys("ps", &["aux"]);
        if !output.success {
            return output;
        }

        let lines: Vec<&str> = output.output.lines().collect();
        let header = lines.first().unwrap_or(&"");
        let data: Vec<&str> = lines[1..].iter()
            .filter(|l| filter.as_ref().map(|f| l.contains(f.as_str())).unwrap_or(true))
            .take(count)
            .copied()
            .collect();

        let result = if data.is_empty() {
            format!("{}", header)
        } else {
            format!("{}\n{}", header, data.join("\n"))
        };

        ToolResult { tool_name: "process_list".into(), output: result, success: true }
    }
}

pub struct EnvVarsTool;

impl Tool for EnvVarsTool {
    fn name(&self) -> &str { "env_vars" }
    fn description(&self) -> &str { "列出环境变量" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "prefix": {"type": "string", "description": "按前缀筛选（如 HERMES、PATH）"}
            },
            "required": []
        })
    }

    fn execute(&self, args: &HashMap<String, String>) -> ToolResult {
        let prefix = args.get("prefix").cloned().unwrap_or_default();
        let vars: Vec<String> = std::env::vars()
            .filter(|(k, _)| prefix.is_empty() || k.starts_with(&prefix))
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        let output = if vars.is_empty() {
            format!("没有匹配 {}* 的环境变量", prefix)
        } else {
            vars.join("\n")
        };
        ToolResult { tool_name: "env_vars".into(), output, success: true }
    }
}

fn run_sys(cmd: &str, args: &[&str]) -> ToolResult {
    match Command::new(cmd).args(args).output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let combined = if stderr.is_empty() { stdout } else { format!("{}\n（stderr）{}", stdout, stderr) };
            ToolResult {
                tool_name: cmd.into(),
                output: if combined.is_empty() { format!("{} 执行完毕（无输出）", cmd) } else { combined },
                success: output.status.success(),
            }
        }
        Err(e) => ToolResult {
            tool_name: cmd.into(),
            output: format!("{} 调用失败: {}", cmd, e),
            success: false,
        },
    }
}
