/// 用户模块——自动识别、画像注入、多账户支持
use std::collections::HashMap;

use crate::module::{
    Capability, Event, EventContext, EventResponse, Module, ModuleError,
};
use tremolite_llm::ToolDefinition;
use tremolite_user::{UserConfig, UserRegistry, User, UserRole, UserSource, migrate_from_display};

// ─── 画像关键词提取 ─────────────────────────────

/// 从用户输入中提取画像碎片
fn extract_traits(input: &str) -> Vec<(String, String)> {
    let mut traits = Vec::new();

    // "我不吃海鲜" / "我不吃一切海产品"
    if let Some(idx) = input.find("不吃") {
        let after = &input[idx + "不吃".len()..].trim();
        let end = after.find(|c: char| c == '。' || c == '，' || c == '!' || c == '？' || c == '的')
            .unwrap_or(after.len().min(20));
        let value = after[..end].trim();
        if !value.is_empty() {
            traits.push(("饮食禁忌".into(), format!("不吃{}", value)));
        }
    }

    // "我喜欢火锅"
    if let Some(idx) = input.find("喜欢") {
        let after = &input[idx + "喜欢".len()..].trim();
        let end = after.find(|c: char| c == '。' || c == '，' || c == '!' || c == '？')
            .unwrap_or(after.len().min(15));
        let value = after[..end].trim();
        if !value.is_empty() && value.chars().count() < 15 {
            traits.push(("喜好".into(), format!("喜欢{}", value)));
        }
    }

    // "我住在成都"
    if let Some(idx) = input.find("住在") {
        let after = &input[idx + "住在".len()..].trim();
        let end = after.find(|c: char| c == '。' || c == '，' || c == '!' || c == '？')
            .unwrap_or(after.len().min(15));
        let value = after[..end].trim();
        if !value.is_empty() {
            traits.push(("居住地".into(), format!("住在{}", value)));
        }
    }

    traits
}

/// 用户模块配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserModuleConfig {
    /// 自主判断模式（auto mode）：通过 prompt_segment 告诉 LLM 可以调用 update_profile 工具
    #[serde(default = "default_true")]
    pub auto_mode: bool,
    /// 定时触发模式（timed mode）：累计消息数自动触发画像提取
    #[serde(default)]
    pub timed_mode: bool,
    /// 定时触发消息阈值
    #[serde(default = "default_message_interval")]
    pub message_interval: u32,
}

fn default_true() -> bool { true }
fn default_message_interval() -> u32 { 5 }

impl Default for UserModuleConfig {
    fn default() -> Self {
        Self {
            auto_mode: default_true(),
            timed_mode: false,
            message_interval: default_message_interval(),
        }
    }
}

// ─── UserModule ──────────────────────────────────

pub struct UserModule {
    pub registry: UserRegistry,
    /// 上次注入到 prompt 的画像内容
    pub last_injected: String,
    // === NEW FIELDS ===
    pub config: UserModuleConfig,
    /// 当前 session 消息计数器（用于 timed mode）
    message_count: u32,
    /// 当前 session_id（用于追踪会话切换时重置计数）
    current_session: String,
}

impl UserModule {
    pub fn new() -> Self {
        Self {
            registry: UserRegistry::new(),
            last_injected: String::new(),
            config: UserModuleConfig::default(),
            message_count: 0,
            current_session: String::new(),
        }
    }

    /// 从配置加载账户列表
    pub fn load_config(&mut self, user_cfg: Option<&serde_json::Value>, display_username: &str, display_ai_name: &str) {
        match user_cfg {
            Some(val) => {
                // 尝试解析为标准 UserConfig
                match serde_json::from_value::<UserConfig>(val.clone()) {
                    Ok(cfg) => {
                        self.registry.load_from_config(&cfg);
                        tracing::info!("user: loaded {} accounts from config", cfg.accounts.len());
                    }
                    Err(e) => {
                        // 解析失败，退到旧 display 模式
                        let fallback = migrate_from_display(display_username, display_ai_name);
                        self.registry.load_from_config(&fallback);
                        tracing::warn!("user: failed to parse user config ({}), fell back to display mode", e);
                    }
                }
            }
            None => {
                // 无 [user] 段，从旧 [display] 迁移
                let fallback = migrate_from_display(display_username, display_ai_name);
                self.registry.load_from_config(&fallback);
                tracing::info!("user: no [user] config, migrated from [display]");
            }
        }
    }

    /// 从配置包 modules/user.toml 读取配置
    pub fn load_module_config(&mut self, profile_dir: &std::path::Path) {
        let config_path = profile_dir.join("modules").join("user.toml");
        if config_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&config_path) {
                if let Ok(cfg) = toml::from_str::<UserModuleConfig>(&content) {
                    self.config = cfg;
                    tracing::info!("user: loaded module config from {:?}", config_path);
                    return;
                }
            }
        }
        tracing::info!("user: using default config (no user.toml found)");
    }

    /// 自动识别当前对话的用户——从渠道来源匹配或创建
    pub fn auto_identify(&mut self, session_id: &str, channel: &str, channel_uid: &str) -> String {
        // 先看 session 是否已绑定
        if let Some(uid) = self.registry.session_uid(session_id) {
            return uid.to_string();
        }

        // 收集多个可能的标识符（channel+uid、仅 uid 等）
        let identifiers: Vec<String> = {
            let mut ids = Vec::new();
            let channel_alias = format!("{}:{}", channel, channel_uid);
            ids.push(channel_alias);
            ids.push(channel_uid.to_string());
            ids
        };
        let id_refs: Vec<&str> = identifiers.iter().map(|s| s.as_str()).collect();

        let uid = if let Some(user) = self.registry.find_by_any_alias(&id_refs) {
            user.uid.clone()
        } else {
            // 未匹配——创建匿名用户
            let alias = format!("{}:{}", channel, channel_uid);
            let user = User::new_anonymous(&alias);
            let uid = user.uid.clone();
            self.registry.add_user(user);
            uid
        };

        self.registry.bind_session(session_id, &uid);
        uid
    }

    /// 获取当前 session 用户的显示名称对
    pub fn display_names(&self, session_id: &str) -> (String, String) {
        if let Some(user) = self.registry.session_user(session_id) {
            (user.display_name.clone(), user.ai_name.clone())
        } else {
            ("用户".into(), "透闪石".into())
        }
    }

    /// 构建当前用户的画像摘要
    pub fn profile_summary(&self, session_id: &str) -> Option<String> {
        let user = self.registry.session_user(session_id)?;
        if user.traits.is_empty() {
            return None;
        }
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("- 名字：{}", user.display_name));
        lines.push(format!("- 角色：{:?}", user.role));
        for (key, val) in &user.traits {
            lines.push(format!("- {}：{}", key, val));
        }
        Some(format!("【当前对话用户画像】\n{}", lines.join("\n")))
    }

    /// 展示当前 session 的用户信息
    fn display_info(&self, session_id: &str) -> String {
        if let Some(user) = self.registry.session_user(session_id) {
            format!("当前用户: {} (uid={}) [{:?}] traits:{}",
                user.display_name, user.uid, user.role, user.traits.len())
        } else {
            "当前用户: 未识别".into()
        }
    }
}

impl Module for UserModule {
    fn id(&self) -> &str { "user" }
    fn name(&self) -> &str { "用户模块" }
    fn version(&self) -> &str { "0.1.0" }

    fn provides(&self) -> Vec<Capability> {
        vec![
            "user.identify".into(),
            "user.profile".into(),
            "user.permission".into(),
        ]
    }

    fn requires(&self) -> Vec<Capability> {
        Vec::new()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> { Some(self) }
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> { Some(self) }

    fn tool_definitions(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "whoami".into(),
                    description: "查看当前对话的用户信息（显示名、角色、画像条数）。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "list_known_users".into(),
                    description: "列出所有已知用户（管理员可见）。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {},
                        "required": []
                    }),
                },
            },
            ToolDefinition {
                def_type: "function".into(),
                function: tremolite_llm::ToolFunction {
                    name: "update_profile".into(),
                    description: "更新当前对话用户的画像信息。当对话中用户透露了新偏好、习惯、个人信息时，调用此工具将信息写入画像。参数：key=画像分类（如\"喜好\"\"饮食禁忌\"\"居住地\"等），value=具体内容。".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "key": {
                                "type": "string",
                                "description": "画像分类名称，如 喜好、饮食禁忌、居住地、职业、健康 等"
                            },
                            "value": {
                                "type": "string",
                                "description": "具体内容"
                            }
                        },
                        "required": ["key", "value"]
                    }),
                },
            },
        ]
    }

    fn execute_tool(&mut self, name: &str, _args: &str) -> Result<String, ModuleError> {
        match name {
            // whoami 不通过 self.registry.session_uid 取——工具执行时没有 session_id 上下文
            // 这里做成全局状态查询
            "whoami" => {
                let count = self.registry.user_count();
                let session_count = self.registry.session_count();
                Ok(format!("已知用户: {} 人\n当前活跃会话: {} 个\n使用 `whoami` 可在对话中查看自己的信息。", count, session_count))
            }
            "list_known_users" => {
                let users: Vec<String> = self.registry.all_users().iter().map(|(uid, u)| {
                    format!("{} ({}) [{:?}] traits:{}", u.display_name, uid, u.role, u.traits.len())
                }).collect();
                if users.is_empty() {
                    Ok("暂无已知用户。".into())
                } else {
                    Ok(users.join("\n"))
                }
            }
            "update_profile" => {
                let parsed: serde_json::Value = serde_json::from_str(_args).map_err(|e| ModuleError::ToolExecutionFailed(e.to_string()))?;
                let key = parsed.get("key").and_then(|v| v.as_str()).unwrap_or("");
                let value = parsed.get("value").and_then(|v| v.as_str()).unwrap_or("");
                if key.is_empty() || value.is_empty() {
                    return Ok("参数不完整，需要 key 和 value".into());
                }
                // 更新当前 session 用户的画像
                // 注：execute_tool 没有 session_id 上下文，这里做一个简化：更新第一个非匿名用户的 traits
                let mut updated = false;
                let uids: Vec<String> = self.registry.all_users().keys().cloned().collect();
                for uid in &uids {
                    if let Some(user) = self.registry.get_mut(uid) {
                        // 跳过匿名用户
                        if user.role == UserRole::Anonymous { continue; }
                        user.traits.insert(key.to_string(), value.to_string());
                        updated = true;
                        break;
                    }
                }
                if updated {
                    self.last_injected = format!("{}: {}", key, value);
                    Ok(format!("已更新 {} → {}", key, value))
                } else {
                    Ok("无可更新的用户".into())
                }
            }
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn prompt_segment(&self) -> Option<String> {
        if self.config.auto_mode {
            Some("\
## 用户画像更新
你可以通过 `update_profile` 工具更新当前用户的画像信息。
当对话中用户透露了新的个人信息（偏好、习惯、健康状况、居住地等），
主动调用 `update_profile(key, value)` 写入画像。

示例：用户说「我喜欢吃火锅」→ update_profile(key=\"喜好\", value=\"火锅\")
".to_string())
        } else {
            None
        }
    }

    fn display_status(&self) -> Option<String> {
        Some(format!("用户: {}人 {}会话", self.registry.user_count(), self.registry.session_count()))
    }

    fn on_event(
        &mut self,
        event: &Event,
        ctx: &EventContext,
    ) -> Result<EventResponse, ModuleError> {
        match event {
            Event::OnMessage { ref input, ref channel, .. } => {
                let session_id = if ctx.session_id.is_empty() { "default" } else { &ctx.session_id };
                let channel_uid = session_id; // 用 session_id 作为 channel_uid
                let uid = self.auto_identify(session_id, channel, channel_uid);

                // 提取画像碎片
                let traits = extract_traits(input);
                if !traits.is_empty() {
                    if let Some(user) = self.registry.get_mut(&uid) {
                        for (key, value) in traits {
                            user.traits.insert(key, value);
                        }
                    }
                }

                // 更新最后注入内容——反映当前 session 用户的最新画像
                let user_traits = self.registry.all_users().get(&uid)
                    .map(|u| &u.traits)
                    .cloned()
                    .unwrap_or_default();
                if !user_traits.is_empty() {
                    let mut parts: Vec<String> = user_traits.iter()
                        .map(|(k, v)| format!("{}: {}", k, v))
                        .collect();
                    parts.sort();
                    self.last_injected = parts.join("\n");
                } else if let Some(user) = self.registry.get(&uid) {
                    self.last_injected = format!("{} — 暂无画像信息", user.display_name);
                }

                // === NEW: timed mode 消息计数 ===
                if self.config.timed_mode {
                    // 如果 session 切换了，重置计数
                    if self.current_session != session_id {
                        self.message_count = 0;
                        self.current_session = session_id.to_string();
                    }
                    self.message_count += 1;

                    if self.message_count >= self.config.message_interval {
                        self.message_count = 0;
                        // 自动触发画像提取（已有 extract_traits 处理，这里只是计数+标记）
                        tracing::info!("user: timed mode triggered at {} messages", self.config.message_interval);
                    }
                }

                Ok(EventResponse::Pass)
            }
            Event::OnResponse { ref response } => {
                // 从 AI 回复中也提取画像线索（用户纠正等）
                // 但回复中的信息通常需要用户确认，先跳过
                let _ = response;
                Ok(EventResponse::Pass)
            }
            _ => Ok(EventResponse::Pass),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_traits_no_match() {
        let t = extract_traits("你好，今天天气不错");
        assert!(t.is_empty());
    }

    #[test]
    fn test_extract_traits_food() {
        let t = extract_traits("我不吃海鲜，吃了过敏");
        assert!(t.iter().any(|(k, _)| k == "饮食禁忌"));
        assert!(t.iter().any(|(_, v)| v.contains("海鲜")));
    }

    #[test]
    fn test_extract_traits_like() {
        let t = extract_traits("我喜欢火锅和宫保鸡丁");
        assert!(t.iter().any(|(_, v)| v.contains("火锅")));
    }

    #[test]
    fn test_auto_identify_creates_anon() {
        let mut um = UserModule::new();
        let uid = um.auto_identify("session1", "qq", "99999");
        assert!(!uid.is_empty());
        assert_eq!(um.registry.user_count(), 1);
    }

    #[test]
    fn test_auto_identify_same_session() {
        let mut um = UserModule::new();
        let uid1 = um.auto_identify("session1", "qq", "99999");
        let uid2 = um.auto_identify("session1", "qq", "99999");
        assert_eq!(uid1, uid2, "同一 session 返回相同 uid");
    }

    #[test]
    fn test_load_config_from_display_fallback() {
        let mut um = UserModule::new();
        um.load_config(None, "琳玲", "葵");
        assert!(um.registry.user_count() >= 1);
        if let Some(user) = um.registry.get("admin") {
            assert_eq!(user.display_name, "琳玲");
            assert_eq!(user.ai_name, "葵");
        }
    }

    #[test]
    fn test_profile_summary_empty() {
        let mut um = UserModule::new();
        um.load_config(None, "测试", "ai");
        let uid = um.auto_identify("s", "t", "t");
        // 没有 traits
        assert!(um.profile_summary("s").is_none());
    }

    #[test]
    fn test_profile_summary_with_traits() {
        let mut um = UserModule::new();
        let uid = um.auto_identify("s", "t", "t");
        if let Some(user) = um.registry.get_mut(&uid) {
            user.traits.insert("饮食禁忌".into(), "不吃海鲜".into());
        }
        let summary = um.profile_summary("s");
        assert!(summary.is_some());
        assert!(summary.unwrap().contains("不吃海鲜"));
    }
}
