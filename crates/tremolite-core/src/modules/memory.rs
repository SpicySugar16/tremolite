use std::any::Any;
use std::collections::HashMap;
use std::path::PathBuf;

use tremolite_memory::{MemoryManager, MemoryStats};
use tremolite_llm::{ToolDefinition, ToolFunction};
use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// 记忆模块——存储、搜索、五层级联代谢
/// 按 session_id 标签隔离不同会话的记忆
pub struct MemoryModule {
    manager: MemoryManager,
}

impl MemoryModule {
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            manager: MemoryManager::new(data_dir.join("memory")),
        }
    }

    fn session_tag(sid: &str) -> String {
        if sid.is_empty() { "session:default".into() } else { format!("session:{}", sid) }
    }

    pub fn stats(&self) -> MemoryStats {
        self.manager.stats()
    }

    pub fn search(&self, query: &str) -> Vec<(tremolite_memory::MemoryLevel, String, f64)> {
        self.manager.search(query)
    }

    pub fn recent_entries(&self, session_id: &str, n: usize) -> Vec<tremolite_memory::MemoryEntry> {
        self.manager.recent_entries(session_id, n)
    }

    pub fn manager(&self) -> &MemoryManager { &self.manager }
    pub fn manager_mut(&mut self) -> &mut MemoryManager { &mut self.manager }

    /// 更新用户画像快速库——写入一条碎片到指定身份
    pub fn update_profile(
        &mut self,
        key: &str,
        _display_name: String,
        traits: std::collections::HashMap<String, String>,
        _source_id: Option<u64>,
        tags: Vec<String>,
        embedding: Option<Vec<f32>>,
    ) {
        let content = traits.values().cloned().collect::<Vec<_>>().join(" | ");
        if !content.is_empty() {
            self.manager.profile_cache.add_entry(key, content, tags, embedding);
        }
    }

    /// 记忆清洗——删除噪音、短条目、命令痕迹，并在内存高时清 L4/L5 缓存
    pub fn decontaminate(&mut self) {
        let stats = self.manager.stats();
        let before = stats.l1_count + stats.l2_count + stats.l3_count + stats.ram_count;

        // 逐 session 清理 L1 中的噪音条目
        let mut total_removed = 0usize;

        for (_sid, buf) in &mut self.manager.l1_sessions {
            let mut removable: Vec<usize> = Vec::new();
            for (i, entry) in buf.entries().iter().enumerate() {
                let trimmed = entry.content.trim();
                let short = trimmed.chars().count() < 5;
                let command_trace = trimmed.starts_with("kamisama: /");
                let noise_only =
                    trimmed.chars().all(|c| c.is_whitespace() || c.is_ascii_punctuation());
                let pure_command = trimmed.starts_with('/') && trimmed.len() < 10;
                if short || command_trace || noise_only || pure_command {
                    removable.push(i);
                }
            }
            // 从后往前删避免索引偏移
            for &i in removable.iter().rev() {
                buf.entries_mut().remove(i);
                total_removed += 1;
            }
        }

        // 清理 ProfileCache 标记
        self.manager.profile_cache.mark_dirty();

        if total_removed > 0 {
            let after = self.manager.stats();
            let total_after = after.l1_count + after.l2_count + after.l3_count + after.ram_count;
            tracing::info!(
                "memory: decontaminate removed {} noise entries ({} → {})",
                total_removed, before, total_after
            );
        } else {
            tracing::info!("memory: decontaminate triggered (before: {} entries, nothing to remove)", before);
        }
    }

    /// 检查当前进程内存，如果超过阈值则自动触发清洗
    /// threshold_mb: 触发清洗的内存阈值（MB）
    pub fn check_memory_pressure(&mut self, threshold_mb: u64) -> bool {
        let rss_kb = get_process_rss_kb();
        match rss_kb {
            Some(kb) => {
                let mb = kb / 1024;
                if mb > threshold_mb {
                    tracing::warn!("memory: pressure detected ({} MB > {} MB), triggering cleanup", mb, threshold_mb);
                    let stats = self.manager.stats();
                    let before = stats.l1_count + stats.l2_count + stats.l3_count + stats.ram_count;
                    // 先做记忆清洗，再将 RAM 中的记忆刷新到磁盘，释放内存
                    self.decontaminate();
                    let _ = self.manager.flush_all();
                    let after = self.manager.stats();
                    let total_before = before;
                    let total_after = after.l1_count + after.l2_count + after.l3_count + after.ram_count;
                    tracing::info!("memory: cleaned ({} -> {} entries)", total_before, total_after);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }
}

impl Module for MemoryModule {
    fn id(&self) -> &str { "memory" }
    fn name(&self) -> &str { "五层记忆" }
    fn version(&self) -> &str { "0.2.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "memory.store".into(),
            "memory.recall".into(),
            "memory.search".into(),
            "memory.metabolize".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> { vec![] }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "search_memory".into(),
                    description: "搜索透闪石的记忆系统，查找过去的对话内容".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "搜索关键词" }
                        },
                        "required": ["query"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "recall_recent".into(),
                    description: "回忆最近几条对话".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "count": { "type": "integer", "description": "条目数，默认5" }
                        },
                        "required": []
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "search_memory" => {
                let parsed: HashMap<String, String> = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
                let query = parsed.get("query").map(|s| s.as_str()).unwrap_or("");
                let results = self.manager.search(query);
                if results.is_empty() {
                    return Ok("没有找到相关记忆呢……".into());
                }
                let lines: Vec<String> = results.iter().take(5).map(|(level, snippet, _score)| {
                    format!("[{}] {}", level.as_str(), snippet)
                }).collect();
                Ok(lines.join("\n"))
            }
            "recall_recent" => {
                let parsed: HashMap<String, serde_json::Value> =
                    serde_json::from_str(args).unwrap_or_default();
                let count = parsed.get("count")
                    .and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                let entries = self.manager.recent_entries("core", count);
                if entries.is_empty() {
                    return Ok("还没有什么记得住的事情呢……".into());
                }
                let lines: Vec<String> = entries.iter().map(|e| {
                    format!("[{}] {}", e.level.as_str(), e.content)
                }).collect();
                Ok(lines.join("\n"))
            }
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn prompt_segment(&self) -> Option<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 读 L2 画像
        let peer = self.manager.l2.all().get("user_peer")
            .map(|e| e.content.clone())
            .unwrap_or_default();

        // 冷/暖启动：最近一条消息的时间（无 session_id 时用空字符串兜底）
        let recent = self.manager.recent_entries("", 5);
        let last_ts = recent.first().map(|e| e.created_at).unwrap_or(0);
        let hours_ago = if last_ts > 0 { (now - last_ts) / 3600 } else { 0 };

        let mut parts: Vec<String> = Vec::new();
        if !peer.is_empty() {
            let truncated: String = peer.chars().take(150).collect();
            parts.push(format!("用户画像：{}", truncated));
        }
        if hours_ago >= 2 && hours_ago > 0 {
            parts.push(format!(
                "（距离上次对话已过约 {} 小时，对方状态可能有变化）", hours_ago
            ));
        }

        if parts.is_empty() { None }
        else { Some(parts.join("\n")) }
    }

    fn on_event(&mut self, event: &Event, ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        let tag = Self::session_tag(&ctx.session_id);
        match event {
            Event::Startup => {
                // 检查 ProfileCache 中是否有已知用户画像
                let has_profile = !self.manager.profile_cache.keys().is_empty();
                if has_profile {
                    if let Some(entries) = self.manager.profile_cache.get_entries("default") {
                        let truncated: String = entries.iter()
                            .flat_map(|e| e.content.chars().take(40))
                            .collect();
                        self.manager.remember(
                            &ctx.session_id,
                            format!("[session_start] 已知用户，画像碎片：{}", truncated),
                            vec!["system".into(), "session_start".into(), tag],
                            0.3, "internal".into(),
                        );
                    }
                }
                // 无 profile 时不写任何 startup 消息——新用户空白开始
                tracing::info!("memory: startup complete (has_profile={})", has_profile);
                Ok(EventResponse::Pass)
            }
            Event::OnMessage { input, channel } => {
                self.manager.remember(
                    &ctx.session_id,
                    format!("kamisama: {}", input),
                    vec![format!("channel:{}", channel), tag],
                    0.6, channel.clone(),
                );
                Ok(EventResponse::Pass)
            }
            Event::OnResponse { response } => {
                self.manager.remember(
                    &ctx.session_id,
                    format!("葵: {}", response),
                    vec!["response".into(), tag], 0.5, "internal".into(),
                );
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                let _ = self.manager.flush_all();
                Ok(EventResponse::Pass)
            }
            Event::Decontaminate => {
                self.decontaminate();
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> { Some(self) }
}

/// 从 /proc/self/status 读取进程 RSS（KB）
fn get_process_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if line.starts_with("VmRSS:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            return parts.get(1).and_then(|v| v.parse::<u64>().ok());
        }
    }
    None
}
