use std::collections::HashMap;

// ─── 数据模型 ─────────────────────────────────────

/// 用户角色
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum UserRole {
    /// 管理员——能改配置、执行危险操作
    Admin,
    /// 普通用户——积累画像，但无系统权限
    User,
    /// 匿名用户——未识别，不积累画像
    Anonymous,
}

impl Default for UserRole {
    fn default() -> Self { Self::Anonymous }
}

/// 用户来源
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum UserSource {
    /// Dashboard 操作 —— 携带选中的账户 uid
    Dashboard { account_uid: String },
    /// QQ 消息
    QQ { qq_id: String },
    /// Webhook
    Webhook { source: String },
    /// TUI
    Tui,
    /// 未知来源
    Unknown,
}

/// 用户
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct User {
    pub uid: String,
    pub display_name: String,
    pub role: UserRole,
    pub avatar: String,
    pub ai_name: String,
    pub ai_avatar: String,
    /// 跨渠道别名（QQ号、webhook来源等）
    pub aliases: Vec<String>,
    /// 画像碎片 key-value
    pub traits: HashMap<String, String>,
    /// 创建时间
    pub created_at: u64,
}

impl User {
    pub fn new_admin(uid: &str, display_name: &str, ai_name: &str) -> Self {
        Self {
            uid: uid.to_string(),
            display_name: display_name.to_string(),
            role: UserRole::Admin,
            avatar: String::new(),
            ai_name: ai_name.to_string(),
            ai_avatar: String::new(),
            aliases: Vec::new(),
            traits: HashMap::new(),
            created_at: now_secs(),
        }
    }

    pub fn new_anonymous(source_alias: &str) -> Self {
        let uid = format!("anon_{}", now_secs());
        Self {
            uid,
            display_name: "访客".into(),
            role: UserRole::Anonymous,
            avatar: String::new(),
            ai_name: "透闪石".into(),
            ai_avatar: String::new(),
            aliases: vec![source_alias.to_string()],
            traits: HashMap::new(),
            created_at: now_secs(),
        }
    }
}

// ─── 配置解析 ─────────────────────────────────────

/// 从 config.toml 解析 [user] 段（含旧 [display] 兼容）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserConfig {
    /// 默认 AI 名称（匿名用户使用）
    #[serde(default = "default_ai_name")]
    pub default_ai_name: String,
    /// 已注册账户列表
    #[serde(default)]
    pub accounts: Vec<AccountConfig>,
}

fn default_ai_name() -> String { "透闪石".into() }

/// 单个账户配置
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AccountConfig {
    pub uid: String,
    #[serde(default = "default_role")]
    pub role: String,
    pub display_name: String,
    pub ai_name: String,
    #[serde(default)]
    pub avatar: String,
    #[serde(default)]
    pub ai_avatar: String,
    /// 绑定的渠道标识（QQ号等），用于自动识别
    #[serde(default)]
    pub aliases: Vec<String>,
}

fn default_role() -> String { "user".into() }

/// 从旧版 [display] 段自动迁移构造 UserConfig
pub fn migrate_from_display(username: &str, ai_name: &str) -> UserConfig {
    UserConfig {
        default_ai_name: ai_name.to_string(),
        accounts: vec![AccountConfig {
            uid: "admin".into(),
            role: "admin".into(),
            display_name: username.to_string(),
            ai_name: ai_name.to_string(),
            avatar: String::new(),
            ai_avatar: String::new(),
            aliases: Vec::new(),
        }],
    }
}

// ─── 用户注册表 ───────────────────────────────────

/// 用户注册表——管理所有已知用户
#[derive(Debug, Clone)]
pub struct UserRegistry {
    /// uid → User
    users: HashMap<String, User>,
    /// alias（QQ号等）→ uid
    alias_index: HashMap<String, String>,
    /// session_id → uid 映射（当前对话绑定）
    session_map: HashMap<String, String>,
}

impl UserRegistry {
    pub fn new() -> Self {
        Self {
            users: HashMap::new(),
            alias_index: HashMap::new(),
            session_map: HashMap::new(),
        }
    }

    /// 从 config 加载账户
    pub fn load_from_config(&mut self, config: &UserConfig) {
        for acct in &config.accounts {
            let role = match acct.role.as_str() {
                "admin" => UserRole::Admin,
                "user" => UserRole::User,
                _ => UserRole::Anonymous,
            };
            let user = User {
                uid: acct.uid.clone(),
                display_name: acct.display_name.clone(),
                role,
                avatar: acct.avatar.clone(),
                ai_name: acct.ai_name.clone(),
                ai_avatar: acct.ai_avatar.clone(),
                aliases: acct.aliases.clone(),
                traits: HashMap::new(),
                created_at: now_secs(),
            };
            // 注册 alias
            for alias in &user.aliases {
                self.alias_index.insert(alias.clone(), user.uid.clone());
            }
            self.users.insert(user.uid.clone(), user);
        }
    }

    /// 通过 alias（QQ号等）查找用户
    pub fn find_by_alias(&self, alias: &str) -> Option<&User> {
        self.alias_index.get(alias).and_then(|uid| self.users.get(uid))
    }

    pub fn get(&self, uid: &str) -> Option<&User> {
        self.users.get(uid)
    }

    pub fn get_mut(&mut self, uid: &str) -> Option<&mut User> {
        self.users.get_mut(uid)
    }

    pub fn add_user(&mut self, user: User) {
        for alias in &user.aliases {
            self.alias_index.insert(alias.clone(), user.uid.clone());
        }
        self.users.insert(user.uid.clone(), user);
    }

    // ─── session ↔ uid 映射 ─────────────────

    pub fn bind_session(&mut self, session_id: &str, uid: &str) {
        self.session_map.insert(session_id.to_string(), uid.to_string());
    }

    pub fn session_user(&self, session_id: &str) -> Option<&User> {
        self.session_map.get(session_id)
            .and_then(|uid| self.users.get(uid))
    }

    pub fn session_uid(&self, session_id: &str) -> Option<&str> {
        self.session_map.get(session_id).map(|s| s.as_str())
    }

    /// 所有已知用户数
    pub fn user_count(&self) -> usize { self.users.len() }

    /// 所有账户（含匿名）
    pub fn all_users(&self) -> &HashMap<String, User> { &self.users }

    /// 活跃 session 数
    pub fn session_count(&self) -> usize { self.session_map.len() }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ─── 单元测试 ─────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_user() {
        let u = User::new_admin("linling", "琳玲", "葵");
        assert_eq!(u.uid, "linling");
        assert_eq!(u.role, UserRole::Admin);
    }

    #[test]
    fn test_anonymous_user() {
        let u = User::new_anonymous("qq:12345");
        assert_eq!(u.role, UserRole::Anonymous);
        assert!(u.aliases.contains(&"qq:12345".to_string()));
    }

    #[test]
    fn test_registry_load_config() {
        let config = UserConfig {
            default_ai_name: "葵".into(),
            accounts: vec![
                AccountConfig {
                    uid: "linling".into(),
                    role: "admin".into(),
                    display_name: "琳玲".into(),
                    ai_name: "葵".into(),
                    avatar: String::new(),
                    ai_avatar: String::new(),
                    aliases: vec!["qq:2513924725".into()],
                },
            ],
        };
        let mut reg = UserRegistry::new();
        reg.load_from_config(&config);
        assert_eq!(reg.user_count(), 1);
        assert!(reg.find_by_alias("qq:2513924725").is_some());
    }

    #[test]
    fn test_migrate_from_display() {
        let config = migrate_from_display("琳玲", "葵");
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].display_name, "琳玲");
        assert_eq!(config.accounts[0].role, "admin");
    }

    #[test]
    fn test_session_binding() {
        let mut reg = UserRegistry::new();
        let u = User::new_admin("test", "测试", "test_ai");
        reg.add_user(u);
        reg.bind_session("session_1", "test");
        assert_eq!(reg.session_uid("session_1"), Some("test"));
        assert_eq!(reg.session_user("session_1").unwrap().display_name, "测试");
    }
}
