# 用户模块设计

## 为什么拆

现在透闪石的用户相关逻辑散落在三处：
- **config.toml display段** — 只支持一对一的 ai_name + username
- **MemoryModule** — 存了两份 String，写 L1 时用来拼 `"kamisama: xxx"` 这种前缀
- **MemoryManager** — ProfileCache 混在记忆代谢引擎里，画像和记忆不分家
- **scheduler.rs** — 硬编码 `"kamisama: "` 当用户前缀

结果就是：一个透闪石实例只能认识一个人，群里其他人来了全算成「kamisama」。不合理。

## 新 crate: `tremolite-user`

独立于记忆系统之外，不依赖内存代谢引擎，有自己的存储和生命周期。

```
tremolite-user/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 模块入口，注册到引擎
│   ├── registry.rs         # 用户注册/识别/跨渠道合并
│   ├── profile.rs          # 画像引擎（偏好/习惯/性格碎片）
│   ├── permission.rs       # 权限层（admin vs user）
│   ├── storage.rs          # 持久化（按 UID 分文件的 JSON store）
│   └── config.rs           # config.toml 的 accounts 段解析
```

## 数据模型

### config.toml 的 accounts 段

```toml
[user]
# 默认账户（用于未登录的 dashboard / 未识别的用户）
default_ai_name = "葵"

[[user.accounts]]
uid = "linling"
role = "admin"
display_name = "琳玲"
ai_name = "葵"
avatar = "data:image/..."
ai_avatar = "data:image/..."

[[user.accounts]]
uid = "another_admin"
role = "admin"
display_name = "某某"
ai_name = "透闪石"
avatar = "..."
ai_avatar = "..."
```

### 运行时用户对象

```rust
pub struct User {
    pub uid: String,           // 唯一标识
    pub display_name: String,  // 界面上显示的名字
    pub role: UserRole,        // admin | user | anonymous
    pub avatar: String,        // 头像 URL
    pub ai_name: String,       // 对这位用户葵自称什么
    pub ai_avatar: String,
    pub source: UserSource,    // 从哪个渠道识别的
    pub aliases: Vec<String>,  // 跨渠道的别名/QQ号
}

pub enum UserRole {
    Admin,     // 能改配置、执行危险操作
    User,      // 普通用户，积累画像
    Anonymous, // 未识别，不积累
}

pub enum UserSource {
    Dashboard { account_uid: String },
    QQ { qq_id: String },
    Webhook { source: String },
    Tui,
    Unknown,
}
```

## 模块接口

### 注册到引擎的 Module trait

```rust
impl Module for UserModule {
    fn id(&self) -> &str { "user" }
    fn provides(&self) -> Vec<Capability> {
        vec!["user.identify", "user.profile", "user.permission"]
    }
}
```

### 核心方法

```rust
impl UserModule {
    /// 根据请求上下文识别用户
    pub fn identify(&self, source: &UserSource) -> &User;

    /// 获取当前对话应该用的 display_name 和 ai_name
    pub fn display_names(&self, uid: &str) -> (String, String);

    /// 写入一条画像碎片
    pub fn add_trait(&mut self, uid: &str, key: &str, value: &str);

    /// 检查权限
    pub fn check_permission(&self, uid: &str, action: &str) -> bool;

    /// 自动识别用户 —— 从消息来源（QQ号/dashboard/webhook）匹配已有用户或创建新用户
    pub fn auto_identify(&self, channel: &str, channel_uid: &str) -> &User;
}
```

### prompt_segment() — 自动画像注入

UserModule 的 `prompt_segment()` 是自动执行的——每次 LLM 调用前，引擎收集所有模块的 prompt 片段时，UserModule 自动把当前对话用户的画像注入：

```rust
fn prompt_segment(&self) -> Option<String> {
    let current_uid = self.current_session_uid()?;
    let profile = self.get_profile(&current_uid)?;
    if profile.is_empty() {
        return None;
    }
    Some(format!("【当前对话用户画像】\n{}", profile))
}
```

注入到 LLM 后的效果——葵知道对面是谁：

```
【当前对话用户画像】
- 名字：琳玲
- 性别：男
- 不吃一切海产品
- 代表色：#bf99bf
- ……（累积的碎片）
```

### 自动识别流程

```
消息进入 → OnMessage 事件
  ↓
UserModule::auto_identify(channel, channel_uid)
  ↓
  在已注册用户中搜索 channel_uid 匹配的 alias
  ├─ 找到 → 返回已有用户，活跃度+1
  └─ 未找到 → 创建新 User(UserRole::User/Anonymous)
             绑定 alias，开始积累画像
  ↓
UserModule 写入：session_id ↔ uid 映射
  ↓
  scheduler 读取历史时通过 UserModule 解析前缀
  MemoryModule 写 L1 时通过 UserModule 获取 display_name
  LLM 调用前 UserModule::prompt_segment() 注入画像
```

### 画像自动积累

OnMessage 和 OnResponse 事件中，UserModule 自动监听并提取画像要素：

```rust
fn on_event(&mut self, event: &Event, ctx: &EventContext) -> Result<...> {
    match event {
        Event::OnMessage { input, channel } => {
            let uid = self.current_uid(&ctx.session_id);
            // 自动提取画像碎片
            for fragment in ProfileExtractor::extract(input) {
                self.add_trait(&uid, &fragment.key, &fragment.value);
            }
        }
        _ => {}
    }
}
```

**提取规则（ProfileExtractor）**：
- 关键词匹配：`我[不]?吃`、`我[不]?喜欢`、`我住在` 等
- LLM 辅助提取：每 N 条消息后批处理分析对话，提炼用户偏好
- 画像置信度递增：同一条信息出现 3 次以上 → 标记为"可信"

### prompt_segment() 注入时机

```
LLM 调用周期：
  1. scheduler 收集所有模块的 prompt_segment()
  2. UserModule 返回当前用户的画像摘要（不超过 300 字）
  3. 拼入 system prompt 末尾
  4. LLM 收到：「你是葵……【当前对话用户画像】……」
```

对于陌生用户（无画像），prompt_segment() 返回 None，不注入任何内容。

## 对现有系统的改动

### 1. config.toml — display 段 → user.accounts

旧：
```toml
[display]
username = "用户"
ai_name = "葵"
```

新：
```toml
[user]
default_ai_name = "葵"

[[user.accounts]]
uid = "linling"
role = "admin"
display_name = "琳玲"
ai_name = "葵"
```

**兼容**：`Config::load()` 检测到旧 `[display]` 段但无 `[user]` 段时，自动迁移构造一个匿名 admin 账户。

### 2. MemoryModule — 不再自己持有 ai_name/user_name

旧：MemoryModule 内部存 `self.ai_name`、`self.user_name`
新：写 L1 时问 UserModule「当前用户什么名字」/「葵对他自称什么」

```rust
// MemoryModule::on_event(OnMessage) 中
let user_module = ctx.get_module("user");
let (uname, _) = user_module.display_names(&ctx.session_uid);
self.manager.remember(&ctx.session_id, format!("{}: {}", uname, input), ...);
```

### 3. scheduler.rs / tui.rs — 不再硬编码 "kamisama: "

旧：
```rust
if let Some(user_msg) = c.strip_prefix("kamisama: ") {
```

新：
```rust
// 从 UserModule 获取当前 session 的用户名
let user_module = engine.get_module("user");
let (uname, aname) = user_module.display_names(&session_uid);
if let Some(user_msg) = c.strip_prefix(&format!("{}: ", uname)) {
    Some(Message::user(user_msg))
} else if let Some(assistant_msg) = c.strip_prefix(&format!("{}: ", aname)) {
    Some(Message::assistant(assistant_msg))
}
```

### 4. Dashboard — 账户选择器

- 左上角加一个账户下拉框（不登录，只是选当前身份）
- 选了之后，整个页面的 `ai_name`/`username`/头像 跟着变
- 配置页编辑 `user.accounts` 列表

### 5. ProfileCache 从 MemoryManager 剥离

旧：ProfileCache 是 MemoryManager 内部的一个字段，和记忆代谢绑在一起
新：画像数据迁移到 UserModule 自己的 `storage.rs`，按 UID 分文件独立存储

```
data/tremolite/users/
├── linling/
│   ├── profile.json        # 画像碎片
│   └── traits.json         # 结构化偏好（key-value）
├── qq_12345/
│   ├── profile.json
│   └── traits.json
└── anonymous/
    └── ...
```

## 实施步骤

| 阶段 | 内容 |
|------|------|
| P0 | 创建 `tremolite-user` crate，实现数据模型 + config 解析 + 账户注册 |
| P1 | 实现自动识别逻辑（`auto_identify`）：从渠道来源匹配或创建用户，绑定 session↔uid |
| P2 | 实现画像引擎 + `prompt_segment` 自动注入（LLM 调用前把当前用户画像注入） + 权限检查 |
| P3 | dashboard 账户选择器 + 多账户 display 联动 |
| P4 | 从 MemoryModule 剥离 ai_name/user_name，改为调用 UserModule |
| P5 | 从 scheduler/tui 移除 "kamisama:" 硬编码，改为 UserModule 动态解析 |
| P6 | ProfileCache 从 MemoryManager 迁移到 UserModule 存储，清理旧数据 |

## 向后兼容

- 没有 `[user]` 段但有 `[display]` 段的旧配置自动迁移
- 旧的 `l2_profile.json` 中的画像数据在 P6 迁移
- 历史 L1 中 `kamisama: xxx` 前缀的条目在 P5 更新解析逻辑后仍能正确回读
