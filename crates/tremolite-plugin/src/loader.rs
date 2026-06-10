use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::io::{BufRead, BufReader, Write};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Plugin, PluginKind, PluginEvent, PluginContext, PluginError};

/// 插件配置（从 config.toml 加载）
#[derive(Debug, Clone, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    #[serde(rename = "type")]
    pub plugin_type: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub enabled: Option<bool>,
}

/// 进程插件的消息协议（JSON 行）
#[derive(Debug, Serialize, Deserialize)]
struct PluginMessage {
    #[serde(rename = "type")]
    msg_type: String,
    payload: Value,
}

/// 进程插件——通过子进程 stdin/stdout JSON 行通信
pub struct ProcessPlugin {
    id: String,
    name: String,
    child: Option<Child>,
}

impl ProcessPlugin {
    pub fn new(id: &str, name: &str, command: &str, args: &[String]) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let child = cmd.spawn()
            .map_err(|e| format!("Failed to start plugin '{}': {e}", name))?;

        tracing::info!("plugin: started '{}' (pid {})", name, child.id());

        Ok(Self {
            id: id.to_string(),
            name: name.to_string(),
            child: Some(child),
        })
    }

    fn send(&mut self, msg: &PluginMessage) -> Result<(), String> {
        if let Some(ref mut child) = self.child {
            let stdin = child.stdin.as_mut()
                .ok_or_else(|| "plugin stdin not available".to_string())?;
            let line = serde_json::to_string(msg)
                .map_err(|e| format!("serialize error: {e}"))?;
            writeln!(stdin, "{line}")
                .map_err(|e| format!("write to plugin stdin: {e}"))?;
            Ok(())
        } else {
            Err("plugin not running".to_string())
        }
    }

    fn recv(&mut self) -> Result<Option<PluginMessage>, String> {
        if let Some(ref mut child) = self.child {
            let stdout = child.stdout.as_mut()
                .ok_or_else(|| "plugin stdout not available".to_string())?;
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => Ok(None), // EOF
                Ok(_) => {
                    let msg: PluginMessage = serde_json::from_str(&line)
                        .map_err(|e| format!("parse plugin message: {e}"))?;
                    Ok(Some(msg))
                }
                Err(e) => Err(format!("read from plugin: {e}")),
            }
        } else {
            Ok(None)
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("plugin: stopped '{}'", self.name);
        }
    }
}

impl Plugin for ProcessPlugin {
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn version(&self) -> &str { "0.1.0" }
    fn kind(&self) -> PluginKind { PluginKind::User }

    fn provides(&self) -> Vec<String> { vec!["process_plugin".into()] }
    fn requires(&self) -> Vec<String> { vec![] }

    fn init(&mut self, ctx: &PluginContext) -> Result<(), PluginError> {
        let msg = PluginMessage {
            msg_type: "init".into(),
            payload: serde_json::json!({
                "capabilities": ctx.capabilities.keys().collect::<Vec<_>>(),
            }),
        };
        self.send(&msg).map_err(|e| PluginError(e))?;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        let msg = PluginMessage {
            msg_type: "shutdown".into(),
            payload: Value::Null,
        };
        let _ = self.send(&msg);
        self.stop();
        Ok(())
    }

    fn on_event(&mut self, event: &PluginEvent, _ctx: &PluginContext) -> Result<Option<crate::PluginAction>, PluginError> {
        let msg_type = match event {
            PluginEvent::Startup => "startup",
            PluginEvent::Shutdown => "shutdown",
            PluginEvent::OnSessionStart => "session_start",
            PluginEvent::PreLlm { .. } => "pre_llm",
            PluginEvent::PostLlm { .. } => "post_llm",
            PluginEvent::OnSessionEnd => "session_end",
        };

        let payload = match event {
            PluginEvent::PreLlm { messages } => {
                serde_json::json!({ "messages": messages })
            }
            PluginEvent::PostLlm { response } => {
                serde_json::json!({ "response": response })
            }
            _ => Value::Null,
        };

        let msg = PluginMessage {
            msg_type: msg_type.into(),
            payload,
        };
        self.send(&msg).map_err(|e| PluginError(e))?;
        Ok(None)
    }
}

impl Drop for ProcessPlugin {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 插件加载器——从配置启动进程插件
pub fn load_plugins(configs: &[PluginConfig]) -> Vec<Box<dyn Plugin>> {
    let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();

    for cfg in configs {
        if !cfg.enabled.unwrap_or(true) {
            tracing::info!("plugin: '{}' disabled, skipping", cfg.name);
            continue;
        }

        match cfg.plugin_type.as_str() {
            "process" => {
                let command = match &cfg.command {
                    Some(cmd) => cmd.clone(),
                    None => {
                        tracing::warn!("plugin '{}': process type requires 'command'", cfg.name);
                        continue;
                    }
                };
                let args = cfg.args.clone().unwrap_or_default();
                match ProcessPlugin::new(&cfg.name, &cfg.name, &command, &args) {
                    Ok(p) => {
                        tracing::info!("plugin: loaded '{}' as process", cfg.name);
                        plugins.push(Box::new(p));
                    }
                    Err(e) => {
                        tracing::error!("plugin: failed to load '{}': {e}", cfg.name);
                    }
                }
            }
            "builtin" => {
                tracing::info!("plugin: '{}' type 'builtin' — must be registered manually", cfg.name);
                // 内置插件由代码直接注册，不走自动加载
            }
            _ => {
                tracing::warn!("plugin: unknown type '{}' for '{}'", cfg.plugin_type, cfg.name);
            }
        }
    }

    plugins
}
