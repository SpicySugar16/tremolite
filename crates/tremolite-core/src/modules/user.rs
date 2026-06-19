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

// ─── UserModule ──────────────────────────────────

pub struct UserModule {
    pub registry: UserRegistry,
}

impl UserModule {
    pub fn new() -> Self {
        Self {
            registry: UserRegistry::new(),
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

    /// 自动识别当前对话的用户——从渠道来源匹配或创建
    pub fn auto_identify(&mut self, session_id: &str, channel: &str, channel_uid: &str) -> String {
        // 先看 session 是否已绑定
        if let Some(uid) = self.registry.session_uid(session_id) {
            return uid.to_string();
        }

        // 对于 QQ 消息，通过 QQ 号匹配 alias
        let alias = if channel == "qq" || channel == "qqbot" {
            format!("qq:{}", channel_uid)
        } else if channel == "dashboard" {
            format!("dashboard:{}", channel_uid)
        } else {
            format!("{}:{}", channel, channel_uid)
        };

        let uid = if let Some(user) = self.registry.find_by_alias(&alias) {
            user.uid.clone()
        } else {
            // 未匹配——创建匿名用户
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
            _ => Err(ModuleError::ToolNotFound(name.to_string())),
        }
    }

    fn prompt_segment(&self) -> Option<String> {
        // prompt_segment 只能通过 &self 访问，不能获取当前 session_id
        // 用户画像注入在 scheduler 层处理，通过 display_names() 和 profile_summary() 传入
        None
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
            Event::OnMessage { ref input, ref channel } => {
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
