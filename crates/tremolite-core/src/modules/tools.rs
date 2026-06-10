use std::collections::HashMap;
use std::any::Any;

use tremolite_llm::{ToolDefinition, ToolFunction};
use tremolite_tools::{ToolRegistry, register_all};
use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// 工具模块——将系统工具打包为 Module，参与引擎生命周期
pub struct ToolsModule {
    registry: ToolRegistry,
    registered: bool,
}

impl ToolsModule {
    pub fn new() -> Self {
        let mut registry = ToolRegistry::new();
        register_all(&mut registry);
        Self {
            registry,
            registered: false,
        }
    }

    pub fn tool_count(&self) -> usize {
        self.registry.list().len()
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.registry.list()
    }
}

impl Module for ToolsModule {
    fn id(&self) -> &str { "tools" }
    fn name(&self) -> &str { "系统工具" }
    fn version(&self) -> &str { "0.2.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "tools.system".into(),
            "tools.filesystem".into(),
            "tools.git".into(),
            "tools.network".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        let mut tools = Vec::new();
        for name in self.registry.list() {
            if let Some(tool) = self.registry.get(&name) {
                let params = tool.parameters();
                tools.push(ToolDefinition {
                    def_type: "function".into(),
                    function: ToolFunction {
                        name: tool.name().into(),
                        description: tool.description().into(),
                        parameters: params,
                    },
                });
            }
        }
        tools
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        let parsed: HashMap<String, String> = if args.trim().is_empty() || args == "{}" {
            HashMap::new()
        } else {
            serde_json::from_str(args)
                .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析失败: {e}")))?
        };

        self.registry.execute(name, &parsed)
            .map(|r| r.output)
            .ok_or_else(|| ModuleError::ToolNotFound(name.to_string()))
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                if !self.registered {
                    self.registered = true;
                    tracing::info!("tools: registered {} system tools", self.registry.list().len());
                }
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { Some(self) }
}
