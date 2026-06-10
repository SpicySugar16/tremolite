//! ## 已废弃
//! 
//! 旧 Plugin trait 已被 `tremolite-module` crate 中的统一 `Module` trait 取代。
//! 进程插件功能已迁移到 `tremolite_module::process_module::ProcessModule`。
//!
//! 本 crate 保留仅用于旧代码的编译兼容，新开发请直接使用 `tremolite-module`。

/// 已废弃——使用 `tremolite_module::Module` 代替
pub trait Plugin: Send + Sync {
    fn id(&self) -> &str { "deprecated" }
    fn name(&self) -> &str { "Deprecated" }
    fn version(&self) -> &str { "0.0.0" }
    fn kind(&self) -> PluginKind { PluginKind::Native }
    fn provides(&self) -> Vec<String> { vec![] }
    fn requires(&self) -> Vec<String> { vec![] }
    fn init(&mut self, _ctx: &PluginContext) -> Result<(), PluginError> { Ok(()) }
    fn shutdown(&mut self) -> Result<(), PluginError> { Ok(()) }
    fn on_event(&mut self, _event: &PluginEvent, _ctx: &PluginContext) -> Result<Option<PluginAction>, PluginError> { Ok(None) }
}

pub enum PluginKind { Native, User, ThirdParty }
pub type Capability = String;
pub enum PluginEvent { Startup, Shutdown, OnSessionStart, PreLlm { messages: Vec<String> }, PostLlm { response: String }, OnSessionEnd }
pub struct PluginContext { pub capabilities: std::collections::HashMap<String, Box<dyn std::any::Any + Send>> }
impl PluginContext { pub fn new() -> Self { Self { capabilities: std::collections::HashMap::new() } } }
pub enum PluginAction { Skip, Rewrite { text: String } }
#[derive(Debug)] pub struct PluginError(pub String);
impl std::fmt::Display for PluginError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "PluginError: {}", self.0) } }
impl std::error::Error for PluginError {}

pub mod loader;
