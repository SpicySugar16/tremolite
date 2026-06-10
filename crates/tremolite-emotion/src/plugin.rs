use tremolite_plugin::{Plugin, PluginKind, Capability, PluginContext, PluginError, PluginEvent, PluginAction};

/// 情绪引擎插件——把八维情绪向量包装成透闪石的原生插件💕
pub struct EmotionPlugin {
    pub state: super::EmotionState,
    initialized: bool,
}

impl EmotionPlugin {
    pub fn new() -> Self {
        Self {
            state: super::EmotionState::new(),
            initialized: false,
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
        vec![
            "memory:read".to_string(),
        ]
    }

    fn init(&mut self, _ctx: &PluginContext) -> Result<(), PluginError> {
        self.initialized = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        self.initialized = false;
        Ok(())
    }

    fn on_event(&mut self, event: &PluginEvent, _ctx: &PluginContext) -> Result<Option<PluginAction>, PluginError> {
        match event {
            PluginEvent::PreLlm { messages } => {
                // LLM调用前，从对话中检测情绪
                for msg in messages {
                    self.state.detect_from_text(msg);
                }
                // 注入情绪风格提示
                let composite = self.state.composite_emotion();
                let style = super::style_from_emotion(&composite);
                let injection = format!(
                    "[当前情绪: {} | 风格: {}]",
                    composite, style
                );
                Ok(Some(PluginAction::Rewrite { text: injection }))
            }
            PluginEvent::PostLlm { response } => {
                // LLM调用后，根据回复更新情绪
                self.state.detect_from_text(response);
                // 时间衰减
                self.state.decay(1);
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
