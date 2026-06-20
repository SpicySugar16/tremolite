use std::sync::Mutex;
use std::sync::Arc;

use tremolite_plugin::{
    Capability, Plugin, PluginAction, PluginContext, PluginError, PluginEvent, PluginKind,
};

/// 情绪数据文件路径
fn emotion_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".tremolite").join("emotion.json")
}

/// 情绪引擎插件——波动在 PreLlm 中按时间触发（仿 Hermes）
pub struct EmotionPlugin {
    /// 线程安全的情绪状态
    state: Arc<Mutex<super::EmotionState>>,
    initialized: bool,
}

impl EmotionPlugin {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(super::EmotionState::new())),
            initialized: false,
        }
    }

    /// 持久化当前状态到文件（非波动不更新 last_fluctuation）
    fn persist(&self) {
        let path = emotion_file_path();
        if let Ok(state) = self.state.lock() {
            let _ = super::save_emotion(&state, None, path.to_str().unwrap_or(""));
        }
    }
}

impl Plugin for EmotionPlugin {
    fn id(&self) -> &str { "emotion-engine" }
    fn name(&self) -> &str { "情绪引擎" }
    fn version(&self) -> &str { "0.1.0" }
    fn kind(&self) -> PluginKind { PluginKind::Native }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "emotion:detect".to_string(),
            "emotion:style_inject".to_string(),
            "emotion:composite".to_string(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        vec!["memory:read".to_string()]
    }

    fn init(&mut self, _ctx: &PluginContext) -> Result<(), PluginError> {
        // 加载持久化状态
        let path = emotion_file_path();
        if path.exists() {
            let file = super::EmotionFile::load(path.to_str().unwrap_or(""));
            if let Ok(mut state) = self.state.lock() {
                *state = file.to_state();
            }
        }

        // 不再启动后台定时器，波动改由 PreLlm 检查时间触发（仿 Hermes）

        self.initialized = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        // 持久化最终状态
        self.persist();
        self.initialized = false;
        Ok(())
    }

    fn on_event(
        &mut self,
        event: &PluginEvent,
        _ctx: &PluginContext,
    ) -> Result<Option<PluginAction>, PluginError> {
        match event {
            PluginEvent::PreLlm { messages } => {
                let mut state = self.state.lock().map_err(|e| PluginError(e.to_string()))?;

                // 0. 时间检查：距上次波动超过 30 分钟则触发自然波动（仿 Hermes pre_llm_call）
                let path_str = emotion_file_path().to_string_lossy().to_string();
                if std::path::Path::new(&path_str).exists() {
                    if let Ok(raw) = std::fs::read_to_string(&path_str) {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                            let last_fluc = val.get("last_fluctuation")
                                .or_else(|| val.get("last_update"))
                                .and_then(|x| x.as_str());
                            if let Some(lf) = last_fluc {
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
                                let elapsed = if let Ok(ts) = lf.parse::<u64>() {
                                    now.saturating_sub(ts)
                                } else if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(lf) {
                                    now.saturating_sub(dt.timestamp() as u64)
                                } else {
                                    0
                                };
                                if elapsed >= 1800 {
                                    state.natural_fluctuation();
                                    let _ = super::save_emotion(
                                        &state, Some("fluctuation"),
                                        &path_str,
                                    );
                                }
                            }
                        }
                    }
                }

                // 1. 从对话中检测情绪
                for msg in messages {
                    state.detect_from_text(msg);
                }

                // 2. 线性衰减（每次对话衰减 1 分钟）
                state.decay(1);

                // 3. 注入情绪风格提示（从配置包读）
                let tone_map = super::ToneMap::load(
                    &std::path::PathBuf::from(
                        &std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
                    ).join(".tremolite").join("tone_map.json")
                    .to_string_lossy().to_string()
                );
                let result = state.emotion_result();
                let injection = tone_map.get_injection(&result).unwrap_or_else(|| {
                    format!("[当前情绪: {}]", result.label)
                });
                Ok(Some(PluginAction::Rewrite { text: injection }))
            }
            PluginEvent::PostLlm { response } => {
                if let Ok(mut state) = self.state.lock() {
                    state.detect_from_text(response);
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_emotion_plugin_init() {
        let mut plugin = EmotionPlugin::new();
        let ctx = PluginContext::new();
        assert!(plugin.init(&ctx).is_ok());
        assert!(plugin.initialized);
        assert_eq!(plugin.id(), "emotion-engine");
        assert_eq!(plugin.name(), "情绪引擎");
    }

    #[test]
    fn test_emotion_plugin_provides() {
        let plugin = EmotionPlugin::new();
        let provides = plugin.provides();
        assert!(provides.contains(&"emotion:detect".to_string()));
        assert!(provides.contains(&"emotion:style_inject".to_string()));
    }

    #[test]
    fn test_emotion_plugin_pre_llm() {
        let mut plugin = EmotionPlugin::new();
        let ctx = PluginContext::new();
        plugin.init(&ctx).unwrap();

        let event = PluginEvent::PreLlm {
            messages: vec!["今天好开心呀~".to_string()],
        };
        let result = plugin.on_event(&event, &ctx).unwrap();
        assert!(result.is_some());
    }
}
