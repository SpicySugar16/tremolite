use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tremolite_plugin::{Capability, Plugin, PluginAction, PluginContext, PluginError, PluginEvent, PluginKind};

/// 波动间隔（秒）——30 分钟
const FLUCTUATION_INTERVAL_SECS: u64 = 1800;

/// 情绪数据文件路径
fn emotion_file_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home).join(".tremolite").join("emotion.json")
}

/// 情绪引擎插件——带独立定时器，每30分钟自动波动💕
pub struct EmotionPlugin {
    /// 线程安全的情绪状态
    state: Arc<Mutex<super::EmotionState>>,
    initialized: bool,
    /// 定时器线程的停止信号
    shutdown_flag: Arc<AtomicBool>,
}

impl EmotionPlugin {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(super::EmotionState::new())),
            initialized: false,
            shutdown_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 持久化当前状态到文件
    fn persist(&self) {
        let path = emotion_file_path();
        if let Ok(state) = self.state.lock() {
            let file = super::EmotionFile {
                plutchik: state.as_plutchik(),
                energy: 50.0,
                last_update: super::now_iso(),
                last_fluctuation: Some(super::now_iso()),
            };
            let _ = file.save(path.to_str().unwrap_or(""));
        }
    }

    /// 启动后台定时波动线程
    fn start_timer(state: Arc<Mutex<super::EmotionState>>, shutdown: Arc<AtomicBool>) {
        thread::spawn(move || {
            loop {
                // 每30秒检查一次停止信号（比直接睡 30min 更优雅）
                for _ in 0..FLUCTUATION_INTERVAL_SECS / 30 {
                    if shutdown.load(Ordering::Relaxed) {
                        return;
                    }
                    thread::sleep(Duration::from_secs(30));
                }

                // 触发自然波动
                if let Ok(mut s) = state.lock() {
                    s.natural_fluctuation();
                    // 持久化
                    let path = emotion_file_path();
                    let file = super::EmotionFile {
                        plutchik: s.as_plutchik(),
                        energy: 50.0,
                        last_update: super::now_iso(),
                        last_fluctuation: Some(super::now_iso()),
                    };
                    let _ = file.save(path.to_str().unwrap_or(""));
                }
            }
        });
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

        // 启动后台定时波动线程
        Self::start_timer(self.state.clone(), self.shutdown_flag.clone());

        self.initialized = true;
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        // 停止定时器
        self.shutdown_flag.store(true, Ordering::Relaxed);
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

                // 1. 从对话中检测情绪
                for msg in messages {
                    state.detect_from_text(msg);
                }

                // 2. 线性衰减（每次对话衰减 1 分钟）
                state.decay(1);

                // 3. 注入情绪风格提示
                let composite = state.composite_emotion();
                let style = super::style_from_emotion(&composite);
                let injection = format!("[当前情绪: {} | 风格: {}]", composite, style);
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
