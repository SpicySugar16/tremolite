# 透闪石成熟路线图 v1.0

> 当前状态：Engine + 调度器 + 模块系统 + 记忆 + 情绪 + 技能 + cron + 斜杠命令 + 模型路由
> 缺口：消息通道、上下文管理、反思自学、技能闭环、Plan/Kanban、热插拔

---

## 一、现状全景

```
当前架构：
  GatewayRouter（仅 CLI）
       ↓
  SessionScheduler（消息路由）
    └─ SessionWorker（x N）
         ├─ OnMessage → 模块广播
         ├─ BuildPrompt → 模块收集
         ├─ process() → LLM + 工具链
         └─ OnResponse → 模块广播 → outbound
  CronModule（独立线程 5s tick → 投递到 scheduler）
  ModelRouter（task_type → provider_name）
```

**已就位的模块（全部实现 Module trait）：**

| 模块 | 行数 | 状态 |
|------|------|------|
| EmotionModule | ✓ | 完整情绪引擎（detect + 复合 + 衰减 + tone_map） |
| MemoryModule | ✓ | 五层存储（L1-L5）+ 代谢 + 检索 + embedding |
| AttentionModule | ✓ | 多尺度注意力 + 关键词奖励 + 实体提取 |
| SkillModule | ✓ | 技能文件加载 + 熟练度 + BuildPrompt 注入 |
| KanbanModule (plan) | ✓ | 计划书创建 + 步骤追踪 + 自动推进 |
| SessionModule | ✓ | 跨 session 互通 + 声明周期两层管理 |
| DelegationModule | ✓ | 子 session 委派（通过调度器） |
| CronModule | ✓ | 独立线程定时 + 投递到调度器 |
| ChannelsModule | ✓ | QQ Bot WebSocket 通道（1114 行，有代码） |

**还缺或只有空壳的：**

| 系统 | 状态 | 说明 |
|------|------|------|
| 消息通道集成 | ⚠️ 代码有但未端到端 | ChannelsModule + WebhookModule 存在，但 CLI 外的真实通道未接通过 |
| 上下文窗口管理 | ❌ | SessionWorker 硬塞 20 条历史，无 token 预算、无压缩、无滑动窗口 |
| 反思系统 | ⚠️ 303 行空壳 | ReflectionModule 注册了但核心逻辑未实现 |
| 自学闭环 | ⚠️ 586 行半成品 | LearningEngine 有技能体系，但 agent 不会自己写技能 |
| 技能热加载 | ❌ | SkillModule 启动时加载技能文件，运行中不检测变更 |
| Plan/Kanban 完整化 | ⚠️ 有架子 | KanbanModule 有 plan 生命周期，但缺自动推进触发回路 |

---

## 二、目标架构（完整 agent）

```
消息通道层（真实平台）
  ├── QQ Bot（已有代码，需端到端验证）
  ├── Telegram（可加，通道 trait 通用）
  ├── Webhook（外部事件接入）
  └── CLI（已有）
       ↓
SessionScheduler（多 session 并发）
  └─ SessionWorker
       ├─ OnMessage → 模块广播
       ├─ BuildPrompt → 模块收集（含技能注入 + 反思画像）
       ├─ Token Budget Check → 超限则 /compress
       ├─ process() → LLM（通过 ModelRouter 选 provider）
       └─ OnResponse → 记忆写入 + 反思触发 + 自学触发

独立子系统：
  ├── CronModule（定时任务）
  ├── PlanModule（计划书自动推进）
  └── ReflectionModule（从对话提取画像 + 检测矛盾 + 更新记忆）
  
自学习闭环：
  对话 → ReflectionModule → 画像更新 → MemoryModule
                         ↘ SkillModule → 技能提议 → 自动编写 → 存档 → BuildPrompt 注入
```

---

## 三、分阶段实施

### Phase A — 消息通道端到端（最高优先）

**目标：** 走通一条真实消息通道。让透闪石收到真实消息、处理、回复。

**现状：** `tremolite-channels` crate 有 1114 行 QQ Bot 实现。但 CLI 外的通道从未被实际调用过。TremoliteEngine::run() 只配了 CLI gateway。

**实施：**

1. `tremolite-channels` crate 检查与当前 Module trait 的兼容性
2. Engine::run() 中通过模块系统启动 ChannelsModule，不再硬编码 gateway
3. 建立测试用的通道适配器（NullGateway 已有）
4. QQ Bot 通道端到端测试：发消息 → 调度器 → worker → LLM → 回复
5. 解决 tokio vs std thread 的兼容问题（channels 用 tokio，引擎用 std thread）

**预估：** 2-3 个会话

### Phase B — 上下文窗口管理

**目标：** SessionWorker 不再硬塞 20 条历史，而是预算驱动地管理上下文。

**现状：** `SessionWorker::process()` 从 MemoryModule 取 `recent_entries(&self.session_id, 20)`，全塞进 prompt。token 超了没有降级路径。

**实施：**

1. 给 `ProviderRegistry` 或 `ModelRouter` 加 token 预算查询能力
2. SessionWorker 在 BuildPrompt 阶段检查总 token，超限时：
   - 保留最近 N 条完整对话
   - 旧对话用 `/compress` 风格的 LLM 调用做单轮总结
   - 把总结塞进 system prompt 的 `[上下文历史摘要]` 段
3. tool call 历史同样做压缩——只保留最近几轮
4. `/compress` 命令从存根变成真实触发压缩的入口
5. 可配置的保留策略（条数优先 / token 优先 / 混合）

**预估：** 2-3 个会话

### Phase C — 反思系统活化

**目标：** ReflectionModule 从空壳变成能从对话中提取画像、检测矛盾、更新用户记忆的子系统。

**现状：** ReflectionModule 注册了但方法主体是空的——只有基本的事件响应框架。

**实施：**

1. 按轮次（每 5 轮对话）或事件（用户纠正）触发反思
2. 反思调用 LLM（通过 ModelRouter 选便宜模型`"reflection"`）分析：
   - 用户偏好和习惯变化
   - 已知事实的矛盾点
   - 葵自身回复的问题（是否幻觉、是否重复）
3. 结果写回 MemoryModule（标签 `"reflection"`）
4. 画像更新——ProfileCache 中的条目验证或修正
5. 结果也通过 BuildPrompt 注入，让葵知道「上一个反思周期发现了什么」

**预估：** 2 个会话

### Phase D — 自学闭环

**目标：** agent 能从对话中自己写技能文件，保存后下次加载。

**现状：** SkillModule 从文件读技能，LearningEngine 有熟练度。但没有「写技能」这个方向。

**实施：**

1. 自学触发器：当观察到某类成功模式重复出现 3 次以上
2. 调用 LLM（通过 ModelRouter 选 `"skill_write"` 模型）写 SKILL.md 格式的技能
3. 保存到 `skills_dir`，SkillModule 自动检测新文件
4. 自我技能排序——新技能初始信任度低，成功使用后提升
5. 技能管理工具：`/skill suggest`、`/skill save`、`/skill prune`

**预估：** 2 个会话

### Phase E — Plan/Kanban 完整化

**目标：** 计划书能自动推进步骤，不再手动 `/next`。

**现状：** KanbanModule 可以创建计划书和步骤，但推进需要外部触发。

**实施：**

1. OnResponse 事件中检查当前是否有活跃计划
2. 活跃计划的当前步骤完成后自动 transition 到下一步
3. 阻塞步骤触发委派子 session
4. 计划完成后自动输出「完成总结」

**预估：** 1 个会话

### Phase F — 技能与模块热插拔

**目标：** 运行中添加/移除技能和模块。

**现状：** 模块在 Engine::new 后注册，之后不能增删。技能文件只在启动时加载。

**实施：**

1. ModuleRegistry 添加 `unregister` 方法
2. SkillModule 添加文件变更监控（inotify/polling）
3. 提供 `/module load <path>`、`/module unload <name>` 工具
4. 模块状态变更后广播 ModuleRegistered/ModuleRemoved 事件

**预估：** 1-2 个会话

---

## 四、依赖关系

```
Phase A（消息通道）—— 无依赖，可最先推
Phase B（上下文管理）—— 依赖 MemoryModule（已有）
Phase C（反思系统）—— 依赖 Phase B（compress 后有稳定上下文才能反思）
Phase D（自学闭环）—— 依赖 SkillModule（已有）+ Phase C（反思发现模式）
Phase E（Plan 完整化）—— 依赖 KanbanModule（已有）
Phase F（热插拔）—— 依赖 ModuleRegistry（已有）+ SkillModule（已有）
```

推荐推进顺序：**A → B → C → D，E 和 F 可与 C/D 并行。**

---

## 五、设计原则

1. **模块间不直接引用**——全部通过 EngineHandle 间接通信
2. **任务类型走模型路由**——轻任务（反思、标题、压缩）走便宜模型，重任务（代码、分析）走强模型
3. **降级优先**——任何新功能如果模型不可用，必须有退化路径
4. **可观测**——所有子系统都在 `/status` 和 `display_status()` 中暴露自己的状态
5. **不破坏已有测试**——每个 Phase 开始前 `cargo test` 绿，完成后全绿
