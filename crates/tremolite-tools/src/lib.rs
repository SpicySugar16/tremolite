use std::collections::HashMap;

// ─── 核心类型 ─────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub tool_name: String,
    pub output: String,
    pub success: bool,
}

/// 工具 trait
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    /// 工具参数的 JSON Schema（OpenAI format），用于 LLM 工具调用
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }
    fn execute(&self, args: &HashMap<String, String>) -> ToolResult;
}

// ─── 工具注册表 ─────────────────────────────────

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: HashMap::new() }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    pub fn get(&self, name: &str) -> Option<&Box<dyn Tool>> {
        self.tools.get(name)
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn execute(&self, name: &str, args: &HashMap<String, String>) -> Option<ToolResult> {
        self.tools.get(name).map(|tool| tool.execute(args))
    }
}

// ─── 内置工具 ─────────────────────────────────────

mod echo;
mod read_file;
mod write_file;
mod shell;
mod http;
mod time;
mod search;
mod cp_file;
mod mv_file;
mod rm_file;
mod append_file;
mod list_dir;
mod glob_files;
mod git;
mod system;
mod diff_files;
mod jq_query;
mod dns_lookup;
mod ping;
mod gh_issue;
mod gh_pr;
mod gh_search;
mod web_search;

pub use echo::EchoTool;
pub use read_file::ReadFileTool;
pub use write_file::WriteFileTool;
pub use shell::ShellTool;
pub use http::HttpTool;
pub use time::TimeTool;
pub use search::SearchTool;
pub use cp_file::CpFileTool;
pub use mv_file::MvFileTool;
pub use rm_file::RmFileTool;
pub use append_file::AppendFileTool;
pub use list_dir::ListDirTool;
pub use glob_files::GlobFilesTool;
pub use git::{GitStatusTool, GitDiffTool, GitLogTool, GitCommitTool, GitPushTool};
pub use system::{DiskUsageTool, MemoryInfoTool, ProcessListTool, EnvVarsTool};
pub use diff_files::DiffFilesTool;
pub use jq_query::JqQueryTool;
pub use dns_lookup::DnsLookupTool;
pub use ping::PingTool;
pub use gh_issue::GhIssueTool;
pub use gh_pr::GhPrTool;
pub use gh_search::GhSearchTool;
pub use web_search::WebSearchTool;

/// 注册所有内置工具
pub fn register_all(registry: &mut ToolRegistry) {
    registry.register(Box::new(EchoTool));
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(ShellTool));
    registry.register(Box::new(HttpTool));
    registry.register(Box::new(TimeTool));
    registry.register(Box::new(SearchTool));
    // Phase 23 — 文件操作增强
    registry.register(Box::new(CpFileTool));
    registry.register(Box::new(MvFileTool));
    registry.register(Box::new(RmFileTool));
    registry.register(Box::new(AppendFileTool));
    registry.register(Box::new(ListDirTool));
    registry.register(Box::new(GlobFilesTool));
    // Phase 23 — Git 工具
    registry.register(Box::new(GitStatusTool));
    registry.register(Box::new(GitDiffTool));
    registry.register(Box::new(GitLogTool));
    registry.register(Box::new(GitCommitTool));
    registry.register(Box::new(GitPushTool));
    // Phase 23 — 系统工具
    registry.register(Box::new(DiskUsageTool));
    registry.register(Box::new(MemoryInfoTool));
    registry.register(Box::new(ProcessListTool));
    registry.register(Box::new(EnvVarsTool));
    // Phase 23 — CLI 工具封装
    registry.register(Box::new(DiffFilesTool));
    registry.register(Box::new(JqQueryTool));
    registry.register(Box::new(DnsLookupTool));
    registry.register(Box::new(PingTool));
    // Phase 30 — 集成工具链
    registry.register(Box::new(GhIssueTool));
    registry.register(Box::new(GhPrTool));
    registry.register(Box::new(GhSearchTool));
    registry.register(Box::new(WebSearchTool));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo_tool() {
        let mut registry = ToolRegistry::new();
        register_all(&mut registry);
        let mut args = HashMap::new();
        args.insert("text".into(), "hello".into());
        let result = registry.execute("echo", &args).unwrap();
        assert!(result.success);
        assert_eq!(result.output, "hello");
    }

    #[test]
    fn test_time_tool() {
        let mut registry = ToolRegistry::new();
        register_all(&mut registry);
        let result = registry.execute("time", &HashMap::new()).unwrap();
        assert!(result.success);
        assert!(result.output.contains("timestamp:"));
        assert!(result.output.contains("unix_epoch:"));
    }

    #[test]
    fn test_unknown_tool() {
        let registry = ToolRegistry::new();
        let result = registry.execute("nonexistent", &HashMap::new());
        assert!(result.is_none());
    }
}
