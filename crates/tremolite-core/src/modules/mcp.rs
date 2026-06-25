use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;

use tremolite_llm::{ToolDefinition, ToolFunction};
use tremolite_mcp::{McpManager, McpServerConfig};
use crate::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError,
};

/// MCP 模块——挂外部 MCP（Model Context Protocol）服务当工具用
///
/// 启动时从配置读取 MCP server 列表，通过 tools/list 发现工具，
/// 把 MCP 工具注册到引擎的工具列表供 LLM 调用。
pub struct McpModule {
    manager: McpManager,
    /// 缓存的工具定义（用于 tool_definitions()）
    tool_defs: Vec<ToolDefinition>,
    /// 工具总数
    tool_count: usize,
    /// 服务器数量
    server_count: usize,
    /// 发现结果缓存写入路径（dashboard 从此文件读展示数据）
    cache_path: Option<PathBuf>,
}

impl McpModule {
    pub fn new() -> Self {
        Self {
            manager: McpManager::new(),
            tool_defs: Vec::new(),
            tool_count: 0,
            server_count: 0,
            cache_path: None,
        }
    }

    /// 设置发现结果缓存路径（dashboard 从此文件读展示数据）
    pub fn with_cache_path(mut self, path: PathBuf) -> Self {
        self.cache_path = Some(path);
        self
    }

    /// 从配置初始化 MCP 服务器
    pub fn with_config(mut self, configs: Vec<McpServerConfig>) -> Self {
        for cfg in configs {
            self.manager.add(cfg);
            self.server_count += 1;
        }
        self
    }

    /// 发现所有 MCP 服务的工具并生成 ToolDefinition
    fn discover_and_register(&mut self) {
        let results = self.manager.discover_all();
        let mut defs = Vec::new();
        let mut cache = serde_json::json!({});

        for (name, prefix, tools, resources, prompts) in &results {
            for tool in tools {
                let tool_name = if prefix.is_empty() {
                    format!("mcp_{}", tool.name)
                } else {
                    format!("mcp_{}_{}", prefix, tool.name)
                };

                defs.push(ToolDefinition {
                    def_type: "function".into(),
                    function: ToolFunction {
                        name: tool_name.clone(),
                        description: format!("[MCP:{}] {}", name, tool.description),
                        parameters: tool.input_schema.clone(),
                    },
                });

                tracing::info!(
                    "mcp: registered tool '{}' from server '{}'",
                    tool_name, name
                );
            }
            self.tool_count += tools.len();

            // 写入发现缓存，供 dashboard API 读取展示
            if let Some(obj) = cache.as_object_mut() {
                obj.insert(name.clone(), serde_json::json!({
                    "tools": tools,
                    "resources": resources,
                    "prompts": prompts,
                }));
            }
        }

        self.tool_defs = defs;

        // 将缓存写入文件
        if let Some(ref cache_path) = self.cache_path {
            if let Some(parent) = cache_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let json = serde_json::to_string_pretty(&cache).unwrap_or_default();
            let _ = std::fs::write(cache_path, json);
            tracing::info!("mcp: wrote discovery cache to {}", cache_path.display());
        }
    }
}

impl Module for McpModule {
    fn id(&self) -> &str {
        "mcp"
    }
    fn name(&self) -> &str {
        "MCP 客户端"
    }
    fn version(&self) -> &str {
        "0.1.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "mcp.tools".into(),
            "mcp.discover".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec![]
    }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tool_defs.clone()
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        // 去掉 mcp_ 前缀定位到实际工具名
        let tool_name = if let Some(stripped) = name.strip_prefix("mcp_") {
            stripped
        } else {
            name
        };

        let arguments: serde_json::Value =
            serde_json::from_str(args).unwrap_or(serde_json::Value::Null);

        match self.manager.call(tool_name, arguments) {
            Ok(result) => Ok(result),
            Err(e) => Err(ModuleError::ToolExecutionFailed(e)),
        }
    }

    fn on_event(
        &mut self,
        event: &Event,
        _ctx: &EventContext,
    ) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                if self.server_count > 0 {
                    tracing::info!(
                        "mcp: connecting to {} MCP servers, discovering tools...",
                        self.server_count
                    );
                    self.discover_and_register();
                    tracing::info!(
                        "mcp: discovered {} tools from {} servers",
                        self.tool_count,
                        self.server_count
                    );
                } else {
                    tracing::info!("mcp: no MCP servers configured");
                }
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                tracing::info!(
                    "mcp: shutting down ({} tools from {} servers)",
                    self.tool_count,
                    self.server_count
                );
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn display_status(&self) -> Option<String> {
        if self.server_count > 0 {
            Some(format!(
                "MCP: {}服务 {}工具",
                self.server_count, self.tool_count
            ))
        } else {
            None
        }
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}
