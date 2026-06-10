use std::any::Any;
use std::collections::HashMap;
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
}

impl EmotionModule {
    pub fn new() -> Self {
        let mut states = HashMap::new();
        states.insert(String::new(), EmotionState::new());
        Self {
            states,
            tone_map: ToneMap::load(""),
            emotion_file_path: String::new(),
        }
    }

    /// 指定 tone_map 路径和 emotion_file 路径
    pub fn with_tone_map(mut self, tone_map_path: &str, emotion_file_path: &str) -> Self {
        self.tone_map = ToneMap::load(tone_map_path);
        self.emotion_file_path = emotion_file_path.to_string();
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
        match event {
            Event::OnMessage { input, .. } => {
                let file_path = self.emotion_file_path.clone();
                let session_id = ctx.session_id.clone();
                let state = self.state_for_mut(&session_id);
                state.detect_from_text(input);

                if !file_path.is_empty() {
                    let file = tremolite_emotion::EmotionFile::from_state(state);
                    let _ = file.save(&file_path);
                }

                Ok(EventResponse::Pass)
            }
            Event::Startup => {
                for state in self.states.values_mut() {
                    state.natural_fluctuation();
                }
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }

    fn as_any(&self) -> Option<&dyn Any> { Some(self) }
}
