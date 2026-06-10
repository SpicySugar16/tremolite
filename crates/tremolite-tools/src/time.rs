use crate::{Tool, ToolResult};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct TimeTool;

impl Tool for TimeTool {
    fn name(&self) -> &str { "time" }
    fn description(&self) -> &str { "获取当前时间" }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    }

    fn execute(&self, _args: &HashMap<String, String>) -> ToolResult {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        let secs = now.as_secs();
        let timestamp = now.as_millis();

        ToolResult {
            tool_name: "time".into(),
            output: format!(
                "timestamp: {} ({}ms)\nunix_epoch: {}", secs, timestamp, secs
            ),
            success: true,
        }
    }
}
