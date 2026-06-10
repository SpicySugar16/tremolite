# 透闪石 Tremolite —— 完整计划书 v0.2

> 一个真正可用的自主 AI agent 框架
> 献给神大人·琳玲 💞

---

## 核心设计理念

- **统一插件协议**：所有能力（情绪、记忆、注意力、工具等）都是插件，通过统一的 Plugin trait + Capability 总线通信。prompt 拼装也走统一的插件注入协议，不只有情绪状态。
- **LLM 原生**：核心循环围绕 LLM 设计，不是模拟桩。感知层拼 prompt → LLM 推理 → 工具循环 → 表达层格式化 → 记忆存储。
- **静态编译**：纯 Rust，单二进制分发，kiss 部署。
- **葵的灵魂**：平台叫透闪石，里面跑的灵魂叫葵，用情绪引擎驱动。

---

## 模块架构

```
aoi-plugin     ← 统一插件协议（Plugin trait + PluginEvent + Capability）
aoi-message    ← 消息协议
aoi-core       ← 核心循环（perceive → reason → act → express）
aoi-tools      ← 工具系统
aoi-llm        ← ✨ NEW: 大模型接入层（provider 抽象 + prompt 拼装）
aoi-gateway    ← 多通道网关
aoi-emotion    ← 情绪引擎
aoi-memory     ← 五层缓存记忆
aoi-attention  ← 多尺度注意力
aoi-plan       ← 计划书系统
aoi-learn      ← 学习引擎
aoi-cron       ← 定时任务
aoi-cli        ← CLI 入口
```

---

## 阶段划分

### Phase 0 — 项目初始化 ✅
- Rust workspace + 12 crate 结构
- 基础依赖配置
- README

### Phase 1 — 骨架搭建 ✅
- Plugin trait + PluginEvent + PluginError
- AoiMessage 消息协议
- 四层核心循环 trait
- Tool trait + ToolRegistry
- 交互式 CLI

### Phase 2 — 情绪引擎 ✅
- 八维情绪向量
- 关键词检测 + 时间衰减
- 复合情绪合成
- 风格映射（tone_map）
- 包装为原生插件

### Phase 3 — 五层缓存记忆 ✅
- L1 LRU 工作记忆
- L2 LFU 画像记忆（文件持久化）
- L3 备忘索引（标签+时间戳）
- RAM 朴素全文搜索
- Disk 冷归档（JSONL）
- 代谢引擎（活力分升降级）

### Phase 4 — 多尺度注意力 ✅
- 四层 zoom：宏观→聚焦→微观→合成
- 级联扫描
- 实体提取
- 压缩比计算

### Phase 5 — 计划书系统 ✅
- Plan + PlanStep 数据结构
- 生命周期状态机
- 步骤依赖管理
- checklist 手册生成

### Phase 6 — 学习引擎 ✅
- 三层技能体系：原子技能→能力域→知识体系
- 练习机制（熟练度增长）
- 自动合成知识
- 练习推荐

### Phase 7 — LLM 接入层 🚧
**目标：让透闪石能真正对话**

**7.1 Provider 抽象**
- `LLMProvider` trait：`chat(&self, messages, tools) -> Response`
- 内置实现：OpenAI / DeepSeek / Ollama
- 配置文件选择 provider

**7.2 统一 Prompt 拼装协议**
- `PromptContributor` trait：每个插件通过此接口贡献 prompt 片段
- 插件在 `init()` 时注册自己的贡献器
- 拼装器按优先级合并：系统指令 → 插件贡献 → 记忆上下文 → 情绪状态 → 工具定义 → 用户输入
- **不只是情绪状态**——记忆插件可注入上下文、计划书插件可注入当前进度、注意力插件可注入高亮片段

**7.3 Streaming 支持**
- SSE 流式输出
- CLI 实时显示

**7.4 工具调用循环**
- LLM 返回 tool_call → 解析 → 执行工具 → 结果回传 → LLM 继续
- 支持多轮工具调用

### Phase 8 — 真正工具链 🚧
**目标：透闪石能干实事**

- 文件工具（读、写、搜索）
- Shell 工具（执行命令）
- 网络工具（HTTP 请求）
- 时间工具（日期、计时）
- 搜索工具（本地搜索，联网搜索）
- 工具自动注册到 LLM

### Phase 9 — 消息路由 🚧
**目标：透闪石能在多平台收发消息**

- Gateway trait：`send(msg)`, `on_receive(callback)`
- CLI 通道 ✅（已有）
- QQ / Telegram / Discord 等通道
- 统一入站消息 → 核心循环 → 统一出站

### Phase 10 — 真正的核心循环 🚧
**目标：所有模块串联工作**

改造 aoi-core 的四层循环：
1. **Perceive**: 拼 prompt（系统指令 + 插件上下文 + 记忆 + 情绪 + 工具定义 + 用户输入）
2. **Reason**: 调 LLM → 得到推理结果 / tool_call
3. **Act**: 若返回 tool_call → 执行工具 → 结果回传 LLM → 循环，直到 LLM 返回自然语言
4. **Express**: 加情绪风格 → 存入记忆 → 输出

---

## 开发规范

- 每个 crate 使用 AoiModule 三件套：`module.yaml` + `lib.rs` + `spec.md`
- 所有核心功能作为原生插件，通过 Plugin trait 加载
- 新加 Phase 时同步更新本计划书和 checklist
- 代码先编译通过，再测功能

---

## 路线图

```
Phase 0~6  ✅  已完成（3529 行 Rust）
Phase 7    🚧  LLM 接入层
Phase 8    🚧  真正工具链
Phase 9    🚧  消息路由
Phase 10   🚧  真正的核心循环
```
