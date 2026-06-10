/// 会话管理——按 session_id 隔离对话状态
///
/// SessionManager 是引擎基础设施，不实现 Module trait。
/// SessionModule 包装在 tremolite-core::modules::session 中。
///
/// 生命周期分两层：
///   冷却（idle 超时）→ 冻结状态，数据保留，不再活跃
///   清理（closed 超时）→ 真正删除，通知 MemoryModule 回收 L1
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 会话状态——引擎通过 session_id 查找
pub struct SessionState {
    pub id: String,
    pub last_active: u64,
    /// 是否允许其他 session 窥探
    pub shared: bool,
    /// 是否已冷却关闭
    pub closed: bool,
    /// 关闭时间戳
    pub closed_at: Option<u64>,
}

impl SessionState {
    pub fn new(id: String) -> Self {
        Self {
            id,
            last_active: now_secs(),
            shared: false,
            closed: false,
            closed_at: None,
        }
    }

    pub fn touch(&mut self) {
        self.last_active = now_secs();
        // touch 时自动激活（如果之前已冷却）
        self.closed = false;
        self.closed_at = None;
    }

    /// 判断是否已过期（超过 ttl 未活跃）
    pub fn is_expired(&self, ttl_secs: u64) -> bool {
        now_secs().saturating_sub(self.last_active) > ttl_secs
    }

    /// 关闭会话——标记为冷却，不删除数据
    pub fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.closed_at = Some(now_secs());
        }
    }

    /// 允许其他 session 查看本 session 的近期对话
    pub fn share(&mut self) {
        self.shared = true;
    }

    /// 拒绝其他 session 查看
    pub fn unshare(&mut self) {
        self.shared = false;
    }
}

/// 会话管理器
pub struct SessionManager {
    sessions: HashMap<String, SessionState>,
    /// 冷却超时（秒）——闲置超过此时间后自动 close
    idle_timeout: u64,
    /// 清理超时（秒）——closed 超过此时间后真正删除
    cleanup_timeout: u64,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            idle_timeout: 300,      // 5 分钟无消息 → 冷却
            cleanup_timeout: 2592000, // 30 天 → 清除 closed
        }
    }

    pub fn with_ttl(ttl_secs: u64) -> Self {
        Self {
            sessions: HashMap::new(),
            idle_timeout: ttl_secs,
            cleanup_timeout: 2592000,
        }
    }

    /// 设置冷却超时
    pub fn set_idle_timeout(&mut self, secs: u64) {
        self.idle_timeout = secs;
    }

    /// 设置清理超时
    pub fn set_cleanup_timeout(&mut self, secs: u64) {
        self.cleanup_timeout = secs;
    }

    pub fn get_or_create(&mut self, id: &str) -> &mut SessionState {
        self.sessions
            .entry(id.to_string())
            .or_insert_with(|| SessionState::new(id.to_string()))
            .touch();
        self.sessions.get_mut(id).unwrap()
    }

    /// 关闭闲置超时的 session——保留数据，仅标记 closed
    /// 返回被冷却的 session_id 列表
    pub fn reap_idle(&mut self) -> Vec<String> {
        let now = now_secs();
        let idle_ids: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| !s.closed && now.saturating_sub(s.last_active) > self.idle_timeout)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &idle_ids {
            if let Some(s) = self.sessions.get_mut(id) {
                s.close();
            }
        }
        idle_ids
    }

    /// 删除 closed 超过 cleanup_timeout 的 session
    /// 返回被彻底清理的 session_id 列表
    pub fn reap_stale_closed(&mut self) -> Vec<String> {
        let now = now_secs();
        let stale: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, s)| {
                s.closed && s.closed_at.map_or(false, |t| now.saturating_sub(t) >= self.cleanup_timeout)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &stale {
            self.sessions.remove(id);
        }
        stale
    }

    /// 关闭所有 session（引擎 shutdown 时用）
    pub fn close_all(&mut self) -> Vec<String> {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        for id in &ids {
            if let Some(s) = self.sessions.get_mut(id) {
                s.close();
            }
        }
        ids
    }

    pub fn count(&self) -> usize {
        self.sessions.len()
    }

    /// 活跃 session 数
    pub fn active_count(&self) -> usize {
        self.sessions.values().filter(|s| !s.closed).count()
    }

    pub fn sessions(&self) -> &HashMap<String, SessionState> {
        &self.sessions
    }

    pub fn sessions_mut(&mut self) -> &mut HashMap<String, SessionState> {
        &mut self.sessions
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let mut mgr = SessionManager::new();
        let s = mgr.get_or_create("test-session");
        assert_eq!(s.id, "test-session");
    }

    #[test]
    fn test_touch_updates_time() {
        let mut mgr = SessionManager::new();
        let s1 = mgr.get_or_create("s1");
        let t1 = s1.last_active;
        std::thread::sleep(Duration::from_millis(10));
        let s2 = mgr.get_or_create("s1");
        assert!(s2.last_active >= t1);
    }

    #[test]
    fn test_close_and_touch_reactivates() {
        let mut mgr = SessionManager::new();
        let s = mgr.get_or_create("s1");
        assert!(!s.closed);

        // 手动 close 后检查状态
        s.close();
        assert!(s.closed);
        assert!(s.closed_at.is_some());

        // touch 应该重新激活
        s.touch();
        assert!(!s.closed, "touch 应该重新激活");
        assert!(s.closed_at.is_none(), "reactivate 后 closed_at 应为 None");
    }

    #[test]
    fn test_reap_idle_after_time() {
        // 通过 close_all + reap_idle 验证 close 后的 inactive 逻辑
        let mut mgr = SessionManager::new();
        mgr.set_idle_timeout(86400); // 一天后才冷却, 确保不会被时间问题影响
        mgr.get_or_create("active");
        // 手动 close 另一个
        let s = mgr.get_or_create("closed_manual");
        s.close();

        // reap_idle 只关闭未closed的, 所以不会影响 closed_manual
        let idle = mgr.reap_idle();
        assert!(idle.is_empty(), "活跃 session 不应该被冷却");
        assert_eq!(mgr.active_count(), 1, "只有 active 一个活跃");
    }

    #[test]
    fn test_stale_removal() {
        let mut mgr = SessionManager::new();
        mgr.set_cleanup_timeout(0); // 0秒后清除, 立刻清理
        let s = mgr.get_or_create("test");
        s.close();

        let purged = mgr.reap_stale_closed();
        assert!(
            purged.contains(&"test".to_string()),
            "closed cleanup_timeout=0 应该立即被清除"
        );
        assert!(mgr.sessions().is_empty(), "清除后不应该有 session");
    }

    #[test]
    fn test_close_all() {
        let mut mgr = SessionManager::new();
        mgr.get_or_create("a");
        mgr.get_or_create("b");
        let ids = mgr.close_all();
        assert_eq!(ids.len(), 2);
        assert!(mgr.sessions().values().all(|s| s.closed));
    }
}
