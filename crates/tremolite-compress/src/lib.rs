use std::any::Any;

use serde::{Deserialize, Serialize};
use tremolite_core::module::{
    Capability, EngineHandle, Event, EventContext, EventResponse, Module, ModuleError,
    ToolDefinition,
};
use tremolite_llm::ToolFunction;
use tremolite_memory::MemoryEntry;

// ─── 压缩比例 ──────────────────────────────────────

/// 单个块的压缩程度
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompressionRatio {
    /// 直接删除整块
    Delete,
    /// 压缩到原长的百分比（0.0~1.0）
    Percent(f64),
    /// 完全不压缩
    Full,
}

impl Default for CompressionRatio {
    fn default() -> Self {
        CompressionRatio::Full
    }
}

/// 块信息——用于显示当前策略
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockConfig {
    pub index: usize,
    pub ratio_label: String,
}

// ─── 压缩策略 ──────────────────────────────────────

/// 压缩策略——控制对话历史如何分块和压缩
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressStrategy {
    pub blocks: usize,
    pub ratios: Vec<CompressionRatio>,
}

impl Default for CompressStrategy {
    fn default() -> Self {
        Self {
            blocks: 5,
            ratios: vec![
                CompressionRatio::Delete,
                CompressionRatio::Percent(0.2),
                CompressionRatio::Percent(0.4),
                CompressionRatio::Percent(0.6),
                CompressionRatio::Full,
            ],
        }
    }
}

impl CompressStrategy {
    pub fn new(blocks: usize, ratios: Vec<CompressionRatio>) -> Result<Self, String> {
        if blocks == 0 {
            return Err("块数必须大于0".into());
        }
        if ratios.len() != blocks {
            return Err(format!("比例数({})必须等于块数({})", ratios.len(), blocks));
        }
        Ok(Self { blocks, ratios })
    }

    pub fn describe(&self) -> Vec<BlockConfig> {
        self.ratios
            .iter()
            .enumerate()
            .map(|(i, r)| BlockConfig {
                index: i,
                ratio_label: match r {
                    CompressionRatio::Delete => format!("块{}: 删除", i),
                    CompressionRatio::Percent(p) => format!("块{}: 压缩到{:.0}%", i, p * 100.0),
                    CompressionRatio::Full => format!("块{}: 不压缩", i),
                },
            })
            .collect()
    }

    pub fn apply<'a, T>(&self, entries: &'a [T]) -> Vec<(usize, &'a T, &CompressionRatio)>
    where
        T: std::fmt::Display,
    {
        if entries.is_empty() {
            return Vec::new();
        }

        let total = entries.len();
        let block_size = (total as f64 / self.blocks as f64).ceil() as usize;
        let mut result = Vec::new();

        for block_idx in 0..self.blocks {
            let start = block_idx * block_size;
            let end = std::cmp::min(start + block_size, total);
            if start >= total {
                break;
            }

            let ratio = &self.ratios[block_idx];
            let block_entries: Vec<&T> = entries[start..end].iter().collect();
            let block_len = block_entries.len();

            match ratio {
                CompressionRatio::Delete => {}
                CompressionRatio::Full => {
                    for (i, entry) in block_entries.iter().enumerate() {
                        result.push((start + i, *entry, ratio));
                    }
                }
                CompressionRatio::Percent(p) => {
                    let keep_count = std::cmp::max(1, (block_len as f64 * p).round() as usize);
                    let keep_start = block_len.saturating_sub(keep_count);
                    for i in keep_start..block_len {
                        result.push((start + i, block_entries[i], ratio));
                    }
                }
            }
        }

        result
    }
}

// ─── Token 估算 ─────────────────────────────────────

/// 粗略估算文本的 token 数
/// 中文约 1.5 chars/token，英文约 3.5 chars/token
/// 混用场景取折中：2 chars/token
pub fn estimate_tokens(text: &str) -> u32 {
    let len = text.len() as f64;
    (len / 2.0).ceil() as u32
}

// ─── CompressModule ──────────────────────────────────

pub struct CompressModule {
    strategy: CompressStrategy,
    compressed_context: String,
    handle: Option<EngineHandle>,
    auto_compress: bool,
    last_compress_count: usize,
    strategy_desc: String,
    /// 上下文压缩阈值（token 数）。0 = 自动，从模型查询
    threshold: u32,
    /// 是否已从 provider 解析过阈值
    threshold_resolved: bool,
}

impl CompressModule {
    pub fn new() -> Self {
        let default_strategy = CompressStrategy::default();
        let desc = Self::format_strategy(&default_strategy);
        Self {
            strategy: default_strategy,
            compressed_context: String::new(),
            handle: None,
            auto_compress: true,
            last_compress_count: 0,
            strategy_desc: desc,
            threshold: 0,
            threshold_resolved: false,
        }
    }

    pub fn set_strategy(&mut self, strategy: CompressStrategy) {
        self.strategy_desc = Self::format_strategy(&strategy);
        self.strategy = strategy;
        self.compressed_context.clear();
    }

    pub fn set_auto_compress(&mut self, enabled: bool) {
        self.auto_compress = enabled;
    }

    pub fn strategy_description(&self) -> &str {
        &self.strategy_desc
    }

    /// 设置压缩阈值（token 数）。传 0 恢复自动。
    pub fn set_threshold(&mut self, tokens: u32) {
        self.threshold = tokens;
        self.threshold_resolved = tokens > 0;
        tracing::info!("compress: threshold set to {} tokens", tokens);
    }

    /// 获取当前有效阈值
    pub fn effective_threshold(&self) -> u32 {
        if self.threshold > 0 {
            self.threshold
        } else {
            // 自动阈值 = 模型上限的一半
            match &self.handle {
                Some(h) => h
                    .get_providers()
                    .map(|p| p.max_context_tokens() / 2)
                    .unwrap_or(64000),
                None => 64000,
            }
        }
    }

    fn format_strategy(strategy: &CompressStrategy) -> String {
        let parts: Vec<String> = strategy
            .describe()
            .iter()
            .map(|b| b.ratio_label.clone())
            .collect();
        format!("{}块: [{}]", strategy.blocks, parts.join(" | "))
    }

    /// 检查当前上下文是否超阈值
    /// 如果超了，就自动压缩直到不超为止
    /// 返回压缩后的文本和是否执行了压缩
    pub fn check_and_compress(&mut self) -> Option<String> {
        if self.compressed_context.is_empty() {
            // 还没压缩过，先压一次
            self.do_compress();
        }

        let threshold = self.effective_threshold();
        let mut rounds = 0;
        let max_rounds = 5;

        loop {
            let text = &self.compressed_context;
            if text.is_empty() {
                return None;
            }

            let tokens = estimate_tokens(text);
            tracing::info!(
                "compress: check_and_compress round {}, {} tokens / {} threshold",
                rounds + 1,
                tokens,
                threshold
            );

            if tokens <= threshold || rounds >= max_rounds {
                break;
            }

            // 重新压缩——在当前结果上再压一次
            tracing::info!(
                "compress: over threshold ({} > {}), re-compressing round {}",
                tokens,
                threshold,
                rounds + 1
            );
            self.do_compress();
            rounds += 1;
        }

        if rounds > 0 {
            Some(format!(
                "上下文超阈值，压缩了 {} 轮。当前 ~{} tokens。",
                rounds,
                estimate_tokens(&self.compressed_context)
            ))
        } else {
            None
        }
    }

    /// 获取压缩后的上下文文本（供 scheduler 集成用）
    pub fn compressed_text(&self) -> &str {
        &self.compressed_context
    }

    /// 用预取的内存条目进行压缩（不从 handle 取数据，避免重入锁）
    pub fn compress_entries(&mut self, entries: &[tremolite_memory::MemoryEntry]) {
        if entries.is_empty() {
            return;
        }

        let texts: Vec<String> = entries.iter().map(|e| e.content.clone()).collect();
        let selected = self.strategy.apply(&texts);

        let mut out = String::from("[上下文压缩]\n");
        for (_, text, ratio) in &selected {
            let label = match ratio {
                CompressionRatio::Full => "",
                CompressionRatio::Percent(p) => &format!("[压缩{:.0}%] ", p * 100.0),
                CompressionRatio::Delete => continue,
            };
            out.push_str(label);
            out.push_str(text);
            out.push('\n');
        }

        if !out.is_empty() {
            let line_count = out.lines().count().saturating_sub(1);
            tracing::info!(
                "compress: {} lines compressed from {} entries",
                line_count,
                entries.len()
            );
            self.compressed_context = out;
        }
    }

    fn do_compress(&mut self) {
        let handle = match &self.handle {
            Some(h) => h.clone(),
            None => return,
        };

        let entries: Vec<tremolite_memory::MemoryEntry> = handle
            .with_module("memory", |m| {
                let mm = match m
                    .as_any()
                    .and_then(|a| a.downcast_ref::<tremolite_core::MemoryModule>())
                {
                    Some(mm) => mm,
                    None => {
                        tracing::warn!("compress: memory module not found");
                        return Vec::new();
                    }
                };
                mm.recent_entries("", 100)
            })
            .unwrap_or_default();

        self.compress_entries(&entries);
    }
}

impl Default for CompressModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for CompressModule {
    fn id(&self) -> &str {
        "compress"
    }

    fn name(&self) -> &str {
        "上下文压缩引擎"
    }

    fn version(&self) -> &str {
        "0.2.0"
    }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "compress.history".into(),
            "compress.strategy".into(),
            "compress.threshold".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        Vec::new()
    }

    fn required_modules(&self) -> Vec<&str> {
        vec!["memory"]
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "compress_strategy_set".into(),
                    description: "设置上下文压缩策略。参数格式 JSON".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "blocks": { "type": "integer", "description": "块数（1-10）", "default": 5 },
                            "ratios": {
                                "type": "array", "items": { "type": "string" },
                                "description": "每块压缩程度，如 [\"Delete\",\"20%\",\"40%\",\"60%\",\"Full\"]"
                            }
                        },
                        "required": ["ratios"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "compress_now".into(),
                    description: "立即执行一次上下文压缩".into(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "compress_set_threshold".into(),
                    description: "设置上下文压缩阈值（token 数），超过此值自动触发压缩。传 0 恢复自动（=模型上限的一半）".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "tokens": { "type": "integer", "description": "阈值 token 数，0=自动" }
                        },
                        "required": ["tokens"]
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "compress_strategy_show".into(),
                    description: "查看当前压缩策略和阈值设置".into(),
                    parameters: serde_json::json!({ "type": "object", "properties": {} }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: ToolFunction {
                    name: "compress_from_entries".into(),
                    description: "用预取的内存条目进行压缩（避免重入锁）".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "entries": {
                                "type": "array",
                                "description": "内存条目列表，每个条目需含 content 字段",
                                "items": { "type": "object" }
                            }
                        },
                        "required": ["entries"]
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, args: &str) -> Result<String, ModuleError> {
        match name {
            "compress_strategy_set" => {
                let parsed: serde_json::Value = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析失败: {}", e)))?;

                let blocks = parsed.get("blocks").and_then(|v| v.as_u64()).unwrap_or(5) as usize;
                let ratios_raw: Vec<String> = parsed
                    .get("ratios")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少 ratios 参数".into()))?;

                let ratios: Result<Vec<CompressionRatio>, String> = ratios_raw
                    .iter()
                    .map(|r| match r.as_str() {
                        "Delete" | "delete" => Ok(CompressionRatio::Delete),
                        "Full" | "full" => Ok(CompressionRatio::Full),
                        s if s.ends_with('%') => {
                            let num: f64 = s.trim_end_matches('%').parse()
                                .map_err(|_| format!("无法解析比例: {}", s))?;
                            Ok(CompressionRatio::Percent(num.clamp(0.0, 100.0) / 100.0))
                        }
                        _ => Err(format!("未知的压缩程度: {}", r)),
                    })
                    .collect();

                let ratios = ratios.map_err(|e| ModuleError::ToolExecutionFailed(e))?;
                let strategy = CompressStrategy::new(blocks, ratios)
                    .map_err(|e| ModuleError::ToolExecutionFailed(e))?;

                self.set_strategy(strategy);
                let desc = self.strategy_description().to_string();
                self.do_compress();
                Ok(format!("策略已更新: {}", desc))
            }

            "compress_now" => {
                let result = self.check_and_compress();
                let lines = self.compressed_context.lines().count().saturating_sub(1);
                match result {
                    Some(msg) => Ok(format!("{}. 缓存 {} 行", msg, lines)),
                    None if lines > 0 => Ok(format!("压缩完成，{} 行上下文。阈值: {} tokens", lines, self.effective_threshold())),
                    None => Ok("没有可压缩的对话历史".into()),
                }
            }

            "compress_set_threshold" => {
                let parsed: serde_json::Value = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析失败: {}", e)))?;
                let tokens = parsed.get("tokens").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                self.set_threshold(tokens);
                if tokens == 0 {
                    Ok(format!("阈值恢复自动: {} tokens", self.effective_threshold()))
                } else {
                    Ok(format!("阈值设为: {} tokens", tokens))
                }
            }

            "compress_strategy_show" => {
                let th = self.effective_threshold();
                let ctx_len = self.compressed_context.len();
                let estimated = estimate_tokens(&self.compressed_context);
                Ok(format!(
                    "策略: {}\n阈值: {} tokens (有效: {})\n压缩缓存: {} 字节 (~{} tokens)",
                    self.strategy_description(),
                    if self.threshold > 0 { self.threshold.to_string() } else { "自动".into() },
                    th,
                    ctx_len,
                    estimated,
                ))
            }

            "compress_from_entries" => {
                let parsed: serde_json::Value = serde_json::from_str(args)
                    .map_err(|e| ModuleError::ToolExecutionFailed(format!("参数解析失败: {e}")))?;
                let entries_arr = parsed.get("entries")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| ModuleError::ToolExecutionFailed("缺少 entries 参数".into()))?;
                let entries: Vec<tremolite_memory::MemoryEntry> = entries_arr
                    .iter()
                    .filter_map(|v| {
                        let content = v.get("content")?.as_str()?;
                        Some(tremolite_memory::MemoryEntry {
                            id: v.get("id").and_then(|v| v.as_u64()).unwrap_or(0),
                            content: content.to_string(),
                            level: tremolite_memory::MemoryLevel::L1,
                            created_at: 0,
                            last_access: 0,
                            access_count: 0,
                            tags: Vec::new(),
                            importance: 0.0,
                            source: String::new(),
                        })
                    })
                    .collect();

                let before = self.compressed_context.lines().count();
                self.compress_entries(&entries);
                let after = self.compressed_context.lines().count();
                let lines_diff = after.saturating_sub(before);
                Ok(format!(
                    "从 {} 条条目压缩。缓存 {} → {} 行，+{} 行。阈值: {} tokens",
                    entries.len(),
                    before, after, lines_diff,
                    self.effective_threshold(),
                ))
            }

            _ => Err(ModuleError::ToolNotFound(name.into())),
        }
    }

    fn prompt_segment(&self) -> Option<String> {
        if self.compressed_context.is_empty() {
            return None;
        }
        let th = self.effective_threshold();
        Some(format!(
            "[压缩上下文]\n以下对话历史已被压缩（策略: {}，阈值: {} tokens）：\n{}",
            self.strategy_desc, th, self.compressed_context
        ))
    }

    fn on_event(&mut self, event: &Event, ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        match event {
            Event::Startup => {
                self.handle = Some(ctx.engine.clone());
                tracing::info!("compress: module started, strategy: {}", self.strategy_desc);
                Ok(EventResponse::Pass)
            }
            Event::OnMessage { .. } => {
                // 注意：不在 OnMessage 中检查压缩，因为 broadcast 持有模块锁，
                // do_compress 内部调用 handle.with_module("memory") 会导致死锁。
                // 压缩检查在 BuildPrompt 阶段完成。
                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                tracing::info!("compress: shutting down");
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn display_status(&self) -> Option<String> {
        Some(format!(
            "压缩: {} {}B (阈值:{})",
            self.strategy_desc,
            self.compressed_context.len(),
            self.effective_threshold(),
        ))
    }
}
