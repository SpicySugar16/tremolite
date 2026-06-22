use tremolite_core::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};
use tremolite_llm::ToolDefinition;

/// 仪表盘模块 — 注册后让 gateway 挂载 Web 管理界面
pub struct DashboardModule {
    #[allow(dead_code)]
    enabled: bool,
}

impl DashboardModule {
    pub fn new() -> Self {
        Self { enabled: true }
    }
}

impl Module for DashboardModule {
    fn id(&self) -> &str { "dashboard" }
    fn name(&self) -> &str { "仪表盘" }
    fn version(&self) -> &str { env!("CARGO_PKG_VERSION") }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "dashboard.ui".into(),
            "dashboard.status".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec![]
    }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    fn prompt_segment(&self) -> Option<String> {
        None
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                tracing::info!("dashboard: 仪表盘已就绪，等待 gateway 挂载 Web 界面");
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }
}

/// Gateway 用这个 HTML 渲染仪表盘界面 — 版本号从 CARGO_PKG_VERSION 动态注入
pub fn dashboard_html() -> String {
    let raw = include_str!("../templates/dashboard.html");
    raw.replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))
}
