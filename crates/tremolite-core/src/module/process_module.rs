use std::collections::HashMap;
use std::process::{Child, Command, Stdio};
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use crate::module::{
    Module, Capability, ModuleError, Event, EventResponse, EventContext,
    ModuleInfo, ToolDefinition, CapabilityDeclaration, EngineEventMessage,
    ModuleEventResponse, ToolCallMessage, ToolResultMessage, ModulePushMessage,
};
use tremolite_llm::ToolFunction;

static PROCESS_SEQ: AtomicU64 = AtomicU64::new(1);

/// 外部进程模块——通过子进程 stdin/stdout JSON 行协议通信
///
/// 任何外部程序（qqbot、slack bridge、自定义工具等）只要实现 JSON 行协议，
/// 就能作为一个 Module 接入透闪石引擎。
pub struct ProcessModule {
    id: String,
    name: String,
    version: String,
    child: Option<Child>,
    reader: Option<BufReader<std::process::ChildStdout>>,
    writer: Option<std::process::ChildStdin>,
    provides: Vec<Capability>,
    requires: Vec<Capability>,
    tools: Vec<ToolDefinition>,
    prompt_content: Option<String>,
    seq: u64,
}

impl ProcessModule {
    /// 启动外部进程并读取能力声明
    pub fn spawn(id: &str, command: &str, args: &[String]) -> Result<Self, String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let mut child = cmd.spawn()
            .map_err(|e| format!("Failed to start process module '{}': {e}", id))?;

        let stdin = child.stdin.take()
            .ok_or_else(|| "stdin not available".to_string())?;
        let stdout = child.stdout.take()
            .ok_or_else(|| "stdout not available".to_string())?;
        let mut reader = BufReader::new(stdout);

        // 读取能力声明（第一行）
        let mut decl_line = String::new();
        reader.read_line(&mut decl_line)
            .map_err(|e| format!("Failed to read capability declaration: {e}"))?;

        let decl: CapabilityDeclaration = serde_json::from_str(&decl_line)
            .map_err(|e| format!("Failed to parse capability declaration: {e} (line: {})", decl_line.trim()))?;

        if decl.msg_type != "capability_declare" {
            return Err(format!("Expected capability_declare, got '{}'", decl.msg_type));
        }

        tracing::info!("module: process '{}' declared {} capabilities, {} tools",
            decl.name, decl.provides.len(), decl.tools.len());

        // 解析工具定义
        let tools: Vec<ToolDefinition> = decl.tools.into_iter().filter_map(|t| {
            let name = t.get("name").and_then(|v| v.as_str())?.to_string();
            let description = t.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let params = t.get("parameters").cloned().unwrap_or(Value::Null);
            Some(ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction { name, description, parameters: params },
            })
        }).collect();

        Ok(Self {
            id: id.to_string(),
            name: decl.name,
            version: decl.version,
            child: Some(child),
            reader: Some(reader),
            writer: Some(stdin),
            provides: decl.provides,
            requires: decl.requires,
            tools,
            prompt_content: decl.prompt_contributions.first().map(|pc| pc.content.clone()),
            seq: PROCESS_SEQ.fetch_add(1, Ordering::Relaxed),
        })
    }

    fn send_event(&mut self, event_type: &str, data: Value) -> Result<(), String> {
        let msg = EngineEventMessage {
            msg_type: "event".into(),
            event: event_type.into(),
            data,
            seq: self.seq,
        };
        let line = serde_json::to_string(&msg)
            .map_err(|e| format!("serialize error: {e}"))?;
        if let Some(ref mut writer) = self.writer {
            writeln!(writer, "{line}")
                .map_err(|e| format!("write to process stdin: {e}"))?;
            writer.flush().ok();
        }
        self.seq += 1;
        Ok(())
    }

    fn recv_response(&mut self) -> Result<Option<ModuleEventResponse>, String> {
        let reader = match self.reader.as_mut() {
            Some(r) => r,
            None => return Ok(None),
        };
        // 非阻塞：使用 BufRead 的 fill_buf 检查是否有数据
        let buf = reader.fill_buf().map_err(|e| format!("read from process: {e}"))?;
        if buf.is_empty() {
            return Ok(None); // EOF — 进程可能已退出
        }

        let mut line = String::new();
        reader.read_line(&mut line)
            .map_err(|e| format!("read line from process: {e}"))?;
        if line.trim().is_empty() {
            return Ok(None);
        }

        // 尝试解析为模块事件响应
        if let Ok(resp) = serde_json::from_str::<ModuleEventResponse>(&line) {
            return Ok(Some(resp));
        }

        // 尝试解析为模块推送消息（上行）
        if let Ok(push) = serde_json::from_str::<ModulePushMessage>(&line) {
            tracing::info!("module: process '{}' pushed event: {:?}", self.id, push);
            // 推送消息暂时只记录日志，未来可加入事件系统
            return Ok(None);
        }

        tracing::warn!("module: process '{}' unknown message: {}", self.id, line.trim());
        Ok(None)
    }

    fn stop(&mut self) {
        let _ = self.send_event("shutdown", Value::Null);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.writer = None;
        self.reader = None;
        tracing::info!("module: process '{}' stopped", self.name);
    }
}

impl Module for ProcessModule {
    fn id(&self) -> &str { &self.id }
    fn name(&self) -> &str { &self.name }
    fn version(&self) -> &str { &self.version }

    fn provides(&self) -> Vec<Capability> { self.provides.clone() }
    fn requires(&self) -> Vec<Capability> { self.requires.clone() }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> { self.tools.clone() }

    fn prompt_segment(&self) -> Option<String> {
        self.prompt_content.clone()
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        let msg = ToolCallMessage {
            msg_type: "tool_call".into(),
            name: name.into(),
            args: serde_json::from_str(args).unwrap_or(Value::Null),
            tool_call_id: format!("proc_{}", self.seq),
        };
        let line = serde_json::to_string(&msg)
            .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;

        if let Some(ref mut writer) = self.writer {
            writeln!(writer, "{line}")
                .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
            writer.flush().ok();
        }

        // 读取工具结果
        let reader = match self.reader.as_mut() {
            Some(r) => r,
            None => return Err(ModuleError::ToolExecutionFailed("process not running".into())),
        };

        let mut result_line = String::new();
        reader.read_line(&mut result_line)
            .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;

        let result: ToolResultMessage = serde_json::from_str(&result_line)
            .map_err(|e| ModuleError::ToolExecutionFailed(format!("parse result: {e}")))?;

        match result.msg_type.as_str() {
            "tool_result" => Ok(result.output.unwrap_or_default()),
            "tool_error" => Err(ModuleError::ToolExecutionFailed(result.error.unwrap_or_default())),
            _ => Err(ModuleError::ToolExecutionFailed(format!("unexpected response type: {}", result.msg_type))),
        }
    }

    fn on_event(&mut self, event: &Event, _ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        let (event_type, data) = match event {
            Event::Startup => ("startup", Value::Null),
            Event::Shutdown => {
                self.stop();
                return Ok(EventResponse::Pass);
            }
            Event::OnMessage { input, channel } => {
                ("on_message", serde_json::json!({ "input": input, "channel": channel }))
            }
            Event::BuildPrompt => ("build_prompt", Value::Null),
            Event::OnToolCall { name, args, success } => {
                ("on_tool_call", serde_json::json!({ "name": name, "args": args, "success": success }))
            }
            Event::OnResponse { response } => {
                ("on_response", serde_json::json!({ "response": response }))
            }
            Event::ModuleRegistered { info } => {
                ("module_registered", serde_json::to_value(info).unwrap_or_default())
            }
            Event::Decontaminate => {("decontaminate", Value::Null)}
        };

        let data_clone = data.clone();
        self.send_event(event_type, data)
            .map_err(|e| ModuleError::EventFailed(e))?;

        // 读取模块的响应（非阻塞）
        if let Ok(Some(resp)) = self.recv_response() {
            match resp.status.as_str() {
                "skip" => return Ok(EventResponse::Skip),
                "modified" => {
                    let modified_data: HashMap<String, Value> =
                        serde_json::from_value(resp.data).unwrap_or_default();
                    // 如果有 prompt_additions，更新 prompt_segment
                    if let Some(additions) = modified_data.get("prompt_additions") {
                        if let Some(arr) = additions.as_array() {
                            if let Some(first) = arr.first() {
                                if let Some(content) = first.get("content").and_then(|v| v.as_str()) {
                                    self.prompt_content = Some(content.to_string());
                                }
                            }
                        }
                    }
                    Ok(EventResponse::Pass)
                }
                _ => Ok(EventResponse::Pass),
            }
        } else {
            Ok(EventResponse::Pass)
        }
    }
}

impl Drop for ProcessModule {
    fn drop(&mut self) {
        self.stop();
    }
}
