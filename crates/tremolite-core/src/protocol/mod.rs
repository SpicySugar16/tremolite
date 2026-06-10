//! # 透闪石模块接入协议
//!
//! 引擎是挖掘机臂，提供液压动力（LLM、调度、记忆等基础服务）。
//! 模块是铲斗、破碎锤、抓木器——通过标准接头（PowerCoupling）接上引擎，
//! 用引擎的动力干自己的活。
//!
//! 模块之间不直接对话。所有交互经过引擎的服务注册表路由。
//!
//! 分层：
//! - `types` — 核心类型：PowerCoupling（液压接头）、ModuleDeclaration、ModuleMessage
//! - `registry` — ServiceRegistry（服务注册表，引擎查谁提供什么服务）

pub mod types;
pub mod registry;

pub use types::*;
pub use registry::ServiceRegistry;
