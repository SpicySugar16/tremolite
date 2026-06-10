//! ## 服务注册表（Service Registry）
//!
//! 引擎的液压分配阀——知道哪个模块提供了什么服务，但不负责转发消息。
//! 引擎主循环直接通过 `ServiceRegistry` 将请求投递给对应的模块处理器。
//!
//! 不是路由器。路由器在模块之间传信，挖掘机的分配阀只是告诉操作员
//! 「铲斗的油管在右边，破碎锤的在左边」——引擎自己开哪个阀门。

use std::collections::HashMap;

use super::types::{
    ModuleDeclaration, ModuleHealth, ModuleStatus, ServiceDefinition,
};

/// 服务注册条目
#[derive(Clone)]
struct ServiceEntry {
    module_id: String,
    service: ServiceDefinition,
}

/// 服务注册表——维护模块与服务的映射
///
/// 引擎在初始化阶段填充此表，运行时查询。
/// 查询返回「提供此服务的模块 ID」——引擎自己决定怎么把请求投递给该模块。
pub struct ServiceRegistry {
    /// 服务名称 → 提供该服务的模块列表
    service_map: HashMap<String, Vec<String>>,
    /// 模块 ID → 模块声明
    declarations: HashMap<String, ModuleDeclaration>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self {
            service_map: HashMap::new(),
            declarations: HashMap::new(),
        }
    }

    /// 注册一个模块及其提供的服务
    pub fn register(&mut self, declaration: ModuleDeclaration) -> Result<(), Vec<String>> {
        let mod_id = declaration.module_id.clone();
        let mut errors = Vec::new();

        // 检查依赖是否满足
        for req in &declaration.requires {
            if !self.service_map.contains_key(&req.name) {
                errors.push(format!(
                    "module '{}' requires service '{}' ({}) which is not provided by any module",
                    mod_id, req.name, req.description
                ));
            }
        }
        for dep_mod in &declaration.required_modules {
            if !self.declarations.contains_key(dep_mod) {
                errors.push(format!(
                    "module '{}' requires module '{}' which is not registered",
                    mod_id, dep_mod
                ));
            }
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        // 注册服务
        for svc in &declaration.provides {
            self.service_map
                .entry(svc.name.clone())
                .or_default()
                .push(mod_id.clone());
        }

        self.declarations.insert(mod_id, declaration);
        Ok(())
    }

    /// 查询提供某服务的模块 ID
    pub fn resolve(&self, service: &str) -> Option<String> {
        self.service_map
            .get(service)
            .and_then(|modules| modules.first().cloned())
    }

    /// 查询所有提供某服务的模块（广播用）
    pub fn resolve_all(&self, service: &str) -> Vec<String> {
        self.service_map.get(service).cloned().unwrap_or_default()
    }

    /// 检查是否存在该服务
    pub fn has_service(&self, service: &str) -> bool {
        self.service_map.contains_key(service)
    }

    /// 获取模块声明
    pub fn get_declaration(&self, module_id: &str) -> Option<&ModuleDeclaration> {
        self.declarations.get(module_id)
    }

    /// 列出所有服务名称
    pub fn list_services(&self) -> Vec<String> {
        self.service_map.keys().cloned().collect()
    }

    /// 列出所有已注册的模块 ID
    pub fn list_modules(&self) -> Vec<String> {
        self.declarations.keys().cloned().collect()
    }

    /// 获取所有模块健康状态
    pub fn all_health(&self) -> Vec<ModuleHealth> {
        self.declarations
            .iter()
            .map(|(id, decl)| ModuleHealth {
                id: id.clone(),
                name: decl.name.clone(),
                version: decl.version.clone(),
                status: ModuleStatus::Running,
                message_count: 0,
                error_count: 0,
                uptime_secs: 0,
                services: decl.provides.iter().map(|s| s.name.clone()).collect(),
                dependencies: decl.requires.iter().map(|s| s.name.clone()).collect(),
                last_error: None,
                details: HashMap::new(),
            })
            .collect()
    }
}
