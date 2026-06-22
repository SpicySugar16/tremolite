use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tremolite_emotion::{EmotionState, ToneMap};
use tremolite_llm::ToolDefinition;
use crate::module::{Module, Capability, ModuleError, Event, EventResponse, EventContext};

/// 情绪模块——检测用户输入中的情绪，生成完整风格注入
/// 使用升级后的 EmotionState（16复合 + 5强度 + tone_map）
/// 内部按 session_id 隔离情绪状态
pub struct EmotionModule {
    states: HashMap<String, EmotionState>,
    tone_map: ToneMap,
    emotion_file_path: String,
    emotion_history_path: String,
    running: Arc<AtomicBool>,
}

impl EmotionModule {
    pub fn new() -> Self {
        let mut states = HashMap::new();
        states.insert(String::new(), EmotionState::new());
        Self {
            states,
            tone_map: ToneMap::load(""),
            emotion_file_path: String::new(),
            emotion_history_path: String::new(),
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 指定 tone_map 路径、emotion_file 路径和 emotion_history 路径
    pub fn with_tone_map(mut self, tone_map_path: &str, emotion_file_path: &str) -> Self {
        self.tone_map = ToneMap::load(tone_map_path);
        self.emotion_file_path = emotion_file_path.to_string();
        // 自动推导 history 路径：将 emotion.json 后缀改为 emotion_history.json
        if emotion_file_path.ends_with("emotion.json") {
            self.emotion_history_path = emotion_file_path
                .strip_suffix("emotion.json")
                .map(|prefix| format!("{}emotion_history.json", prefix))
                .unwrap_or_else(|| format!("{}_history", emotion_file_path));
        } else {
            self.emotion_history_path = format!("{}_history", emotion_file_path);
        }
        // 如果 emotion_file 存在，从文件恢复状态
        if !emotion_file_path.is_empty() {
            let file = tremolite_emotion::EmotionFile::load(emotion_file_path);
            let state = file.to_state();
            self.states.insert(String::new(), state);
        }
        self
    }

    fn state_for(&self, sid: &str) -> &EmotionState {
        self.states.get(sid).unwrap_or_else(|| {
            // fallback to default session
            self.states.get("").expect("EmotionModule: default session missing")
        })
    }

    fn state_for_mut(&mut self, sid: &str) -> &mut EmotionState {
        let sids = sid.to_string();
        self.states.entry(sids).or_insert_with(EmotionState::new)
    }

    pub fn composite_emotion(&self) -> String {
        self.state_for("").composite_emotion()
    }

    fn persist_all(&self, source: Option<&str>) {
        if let Some(state) = self.states.get("") {
            if !self.emotion_file_path.is_empty() {
                let _ = tremolite_emotion::save_emotion(state, source, &self.emotion_file_path);
            }
        }
    }

    pub fn emotion_state(&self) -> &EmotionState {
        self.state_for("")
    }

    pub fn emotion_state_mut(&mut self) -> &mut EmotionState {
        self.state_for_mut("")
    }

    /// 获取TUI状态栏显示的紧凑情绪文本
    pub fn display_status(&self) -> String {
        let result = self.state_for("").emotion_result();
        let emoji = self.tone_map.entries.get(&result.label)
            .and_then(|e| e.levels.get(result.intensity.as_str()))
            .and_then(|l| l.emoji.as_deref())
            .unwrap_or("");
        if emoji.is_empty() {
            format!("{}·{}", result.label, result.intensity.as_str())
        } else {
            format!("{}·{} {}", result.label, result.intensity.as_str(), emoji)
        }
    }
}

impl Module for EmotionModule {
    fn id(&self) -> &str { "emotion" }
    fn name(&self) -> &str { "情绪引擎" }
    fn version(&self) -> &str { "0.3.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "emotion.detect".into(),
            "emotion.style".into(),
            "emotion.composite".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec![]
    }
    fn required_modules(&self) -> Vec<&str> { vec![] }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }

    /// prompt_segment：生成完整风格注入文本
    fn prompt_segment(&self) -> Option<String> {
        let result = self.state_for("").emotion_result();

        if let Some(injection) = self.tone_map.get_injection(&result) {
            return Some(injection);
        }

        let style = tremolite_emotion::style_from_emotion(&result.label);
        Some(format!(
            "[当前情绪]\\n状态: {}\\\n强度: {}\\\n风格: {}",
            result.label,
            result.intensity.as_str(),
            style,
        ))
    }

    fn display_status(&self) -> Option<String> {
        Some(self.display_status())
    }

    fn on_event(&mut self, event: &Event, ctx: &EventContext) -> Result<EventResponse, ModuleError> {
        let file_path = self.emotion_file_path.clone();
        let history_path = self.emotion_history_path.clone();
        match event {
            Event::OnMessage { input, .. } => {
                let session_id = ctx.session_id.clone();
                let state = self.state_for_mut(&session_id);

                // 检测前保存状态副本以判断是否有变化
                let old_result = state.emotion_result();
                state.detect_from_text(input);
                let new_result = state.emotion_result();

                if !file_path.is_empty() {
                    let _ = tremolite_emotion::save_emotion(state, None, &file_path);
                }

                // 如果情绪标签或强度有变化，追加 manual 历史
                if old_result.label != new_result.label || old_result.intensity.as_str() != new_result.intensity.as_str() {
                    if !history_path.is_empty() {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                        let entry = serde_json::json!({
                            "timestamp": now,
                            "type": "manual",
                            "plutchik": {
                                "joy": state.joy, "sadness": state.sadness, "anger": state.anger,
                                "fear": state.fear, "surprise": state.surprise, "disgust": state.disgust,
                                "anticipation": state.anticipation, "trust": state.trust,
                            },
                            "style": new_result.label,
                        });
                        let _ = tremolite_emotion::append_history(&history_path, &entry);
                    }
                }

                Ok(EventResponse::Pass)
            }
            Event::Startup => {
                for state in self.states.values_mut() {
                    state.natural_fluctuation();
                }
                self.persist_all(Some("fluctuation"));

                // 启动后台定时波动线程
                if !self.running.load(Ordering::Relaxed) {
                    self.running.store(true, Ordering::Relaxed);
                    let running = self.running.clone();
                    let fp = file_path.clone();
                    let hp = self.emotion_history_path.clone();

                    thread::spawn(move || {
                        while running.load(Ordering::Relaxed) {
                            thread::sleep(Duration::from_secs(60));

                            // 从文件读取状态 → 波动 → 写回
                            let mut state = EmotionState::new();
                            if !fp.is_empty() {
                                let f = tremolite_emotion::EmotionFile::load(&fp);
                                state = f.to_state();
                            }

                            // 读取自动波动间隔（从文件动态获取）
                            let interval = std::fs::read_to_string(&fp).ok()
                                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                .and_then(|v| v.get("auto_fluctuation_seconds").and_then(|x| x.as_f64()))
                                .unwrap_or(1800.0) as u64;

                            // 检查是否到波动时间
                            let last_fluc = if !fp.is_empty() {
                                std::fs::read_to_string(&fp).ok()
                                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                                    .and_then(|v| {
                                        v.get("last_fluctuation")
                                            .or_else(|| v.get("last_update"))
                                            .and_then(|x| x.as_str().map(String::from))
                                    })
                            } else {
                                None
                            };

                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                            let elapsed = match &last_fluc {
                                Some(lf) if let Ok(ts) = lf.parse::<u64>() => now.saturating_sub(ts),
                                Some(lf) if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(lf) => now.saturating_sub(dt.timestamp() as u64),
                                _ => u64::MAX,
                            };

                            if elapsed >= interval {
                                state.natural_fluctuation();
                                if !fp.is_empty() {
                                    let _ = tremolite_emotion::save_emotion(&state, Some("fluctuation"), &fp);
                                }
                                // 追加波动历史
                                if !hp.is_empty() {
                                    let result = state.emotion_result();
                                    let entry = serde_json::json!({
                                        "timestamp": now,
                                        "type": "fluctuation",
                                        "plutchik": {
                                            "joy": state.joy, "sadness": state.sadness, "anger": state.anger,
                                            "fear": state.fear, "surprise": state.surprise, "disgust": state.disgust,
                                            "anticipation": state.anticipation, "trust": state.trust,
                                        },
                                        "style": result.label,
                                    });
                                    let _ = tremolite_emotion::append_history(&hp, &entry);
                                }
                            }
                        }
                    });
                }

                Ok(EventResponse::Pass)
            }
            Event::Shutdown => {
                self.running.store(false, Ordering::Relaxed);
                self.persist_all(None);
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
}
