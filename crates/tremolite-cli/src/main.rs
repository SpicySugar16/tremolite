use std::path::PathBuf;
use std::sync::Arc;

use tremolite_core::TremoliteEngine;
use tremolite_core::{
    AttentionModule, CronModule, DelegationModule, EmotionModule, KanbanModule,
    McpModule, MemoryModule, SessionModule, SkillModule, ToolsModule, UserModule, WebhookModule,
};
use tremolite_channels::ChannelsModule;
use tremolite_compress::CompressModule;
use tremolite_config::Config;
use tremolite_dashboard::DashboardModule;
use tremolite_reflection::ReflectionModule;
use tremolite_server::{initialize_channels, run_server};
use tremolite_channels::ChannelRegistry;

mod cli;
mod tui;

const VERSION: &str = "0.3.0";

/// 从 .env 文件加载环境变量
fn load_dotenv(path: &std::path::Path) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(eq) = trimmed.find('=') {
            let key = trimmed[..eq].trim();
            let value = trimmed[eq + 1..].trim();
            if !key.is_empty() && std::env::var(key).is_err() {
                std::env::set_var(key, value);
            }
        }
    }
}

fn main() {
    // ── 0. 解析子命令 ────────────────────────────
    let parsed = cli::parse_args();

    // ── Delegate 快速路径：不启动日志系统 ────────
    // tracing 默认写 stdout，会污染 IPC 协议的第一行结果
    // 所以在 tracing 初始化前先拦截 Delegate 模式
    if matches!(parsed.subcommand, cli::Subcommand::Delegate { .. }) {
        handle_delegate();
        return;
    }

    let log_level = &parsed.log_level;

    // ── 初始化日志系统 ────────────────────────────
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        use tracing_subscriber::EnvFilter;

        let log_dir = std::env::current_dir()
            .unwrap_or_default()
            .join("logs");
        let _ = std::fs::create_dir_all(&log_dir);

        let is_daemon = matches!(parsed.subcommand, cli::Subcommand::Daemon { .. });
        let stdout_layer = tracing_subscriber::fmt::layer()
            .with_target(false)
            .with_thread_ids(false)
            .with_ansi(!is_daemon);

        let file_appender = tracing_appender::rolling::hourly(&log_dir, "tremolite.log");
        let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
        let file_layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(non_blocking);

        let filter = EnvFilter::try_new(log_level)
            .unwrap_or_else(|_| EnvFilter::new("info"));

        tracing_subscriber::registry()
            .with(filter)
            .with(stdout_layer)
            .with(file_layer)
            .init();

        tracing::info!("Tremolite v{} starting", VERSION);
    }

    // ── 快速子命令（不需要引擎初始化的）────────────
    match &parsed.subcommand {
        cli::Subcommand::Version => {
            println!("Tremolite v{}", VERSION);
            return;
        }
        cli::Subcommand::Help => {
            cli::print_help();
            return;
        }
        cli::Subcommand::Delegate { .. } => {
            handle_delegate();
            return;
        }
        _ => {}
    }

    // ── 需要引擎初始化的子命令 ─────────────────
    let port = match &parsed.subcommand {
        cli::Subcommand::Daemon { port, .. } | cli::Subcommand::Dashboard { port, .. } => *port,
        _ => 8080,
    };

    // ── 1. 自动加载环境变量 ────────────────────────
    if let Ok(home) = std::env::var("HOME") {
        let hermes_env = std::path::PathBuf::from(&home).join(".hermes").join(".env");
        load_dotenv(&hermes_env);
    }
    load_dotenv(&std::path::PathBuf::from(".env"));

    // ── 2. 加载配置 ──────────────────────────────
    let config = match Config::load(parsed.config_path.as_deref()) {
        Ok(cfg) => {
            println!("  Config loaded from config.toml ✓");
            Some(cfg)
        }
        Err(e) => {
            println!("  No config file found: {}", e);
            println!("  Running in offline mode");
            None
        }
    };

    // ── 处理纯配置查询子命令（plan/tool/skill/config/health/session）─
    // 这些子命令只需要配置和数据目录，不需要完整引擎
    match &parsed.subcommand {
        cli::Subcommand::Plan { action, args } => {
            handle_plan(action, args, config.as_ref());
            return;
        }
        cli::Subcommand::Tool { action, args } => {
            handle_tool(action, args);
            return;
        }
        cli::Subcommand::Skill { action, args } => {
            handle_skill(action, args, config.as_ref());
            return;
        }
        cli::Subcommand::Config { action, args } => {
            handle_config(action, args, config.as_ref());
            return;
        }
        cli::Subcommand::Health => {
            handle_health(config.as_ref());
            return;
        }
        cli::Subcommand::Session { action, args } => {
            handle_session(action, args, config.as_ref());
            return;
        }
        cli::Subcommand::Module { action, args } => {
            handle_module(action.to_string(), args.clone(), config.as_ref());
            return;
        }
        _ => {}
    }

    // ── 运行模式子命令需要完整引擎启动 ──────────────
    let data_dir = config.as_ref()
        .map(|c| PathBuf::from(&c.core.data_dir))
        .unwrap_or_else(|| PathBuf::from("./data/tremolite"));
    // 配置包路径——所有 agent 独属数据从包目录读
    let profile_dir = config.as_ref()
        .map(|c| c.profile.path())
        .unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".tremolite").join("profiles").join("main")
        });

    let mut engine = TremoliteEngine::new(data_dir.clone());
    engine.session_id = parsed.session_id.clone();

    // ── 3. 初始化 LLM 提供者 ─────────────────
    if let Some(ref cfg) = config {
        match cfg.initialize_providers() {
            Ok(registry) => {
                engine.set_providers(std::sync::Arc::new(registry));
                println!("  LLM providers initialized ✓");
            }
            Err(e) => {
                eprintln!("  Warning: Failed to initialize LLM providers: {}", e);
            }
        }
        // 从配置包读 SOUL.md
        let soul_path = cfg.profile.soul_path();
        let soul = if soul_path.exists() {
            std::fs::read_to_string(&soul_path).unwrap_or_default()
        } else {
            cfg.core.soul()
        };
        if !soul.is_empty() {
            engine.set_soul(&soul);
        }
    }

    // ── 4. 注册所有模块 ──────────────────────────
    let d = data_dir.clone();

    // 读取用户配置（用户名/AI名）
    let (ai_name, username) = {
        let home = std::env::var("HOME").unwrap_or_default();
        let profile_path = std::path::Path::new(&home).join(".tremolite/profiles/aoi/profile.json");
        if let Ok(content) = std::fs::read_to_string(&profile_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) {
                (
                    json.get("ai_name").and_then(|v| v.as_str()).unwrap_or("葵").to_string(),
                    json.get("username").and_then(|v| v.as_str()).unwrap_or("琳玲").to_string(),
                )
            } else {
                ("葵".to_string(), "琳玲".to_string())
            }
        } else {
            ("葵".to_string(), "琳玲".to_string())
        }
    };
    // 模块资源路径从配置包读
    let tm_path = profile_dir.join("tone_map.json").to_string_lossy().to_string();
    let em_path = profile_dir.join("emotion.json").to_string_lossy().to_string();
    let _ = engine.register_module(Box::new(EmotionModule::new()
        .with_tone_map(&tm_path, &em_path)));

    // 系统工具模块——将内置工具注册到模块系统
    let tools_module = ToolsModule::new(Some(profile_dir.clone()));
    let tool_count = tools_module.tool_count();
    let _ = engine.register_module(Box::new(tools_module));
    println!("  Tools registered: {} ✓", tool_count);

    let _ = engine.register_module(Box::new(DashboardModule::new()));

    let memory_data_base = profile_dir.join("data");
    if !memory_data_base.exists() {
        std::fs::create_dir_all(&memory_data_base).unwrap_or(());
    }
    let mut memory_module = MemoryModule::new(memory_data_base).with_names(&ai_name, &username);
    // 从 profile 读 BGE 嵌入配置
    let embedding_cfg_path = profile_dir.join("variables.toml");
    if let Ok(content) = std::fs::read_to_string(&embedding_cfg_path) {
        if let Ok(toml_val) = content.parse::<toml::Value>() {
            if let Some(tbl) = toml_val.get("api") {
                let sf_key_raw = tbl.get("siliconflow_key").and_then(|v|v.as_str()).unwrap_or("");
                let sf_key = if sf_key_raw.starts_with("${") {
                    let var_name = sf_key_raw.trim_start_matches("${").trim_end_matches('}');
                    std::env::var(var_name).unwrap_or_default()
                } else {
                    sf_key_raw.to_string()
                };
                let sf_url = tbl.get("siliconflow_url").and_then(|v|v.as_str()).unwrap_or("https://api.siliconflow.cn/v1");
                let sf_model = tbl.get("siliconflow_embedding_model").and_then(|v|v.as_str()).unwrap_or("BAAI/bge-large-zh-v1.5");
                if !sf_key.is_empty() {
                    let emb_url = format!("{}/embeddings", sf_url.trim_end_matches('/'));
                    memory_module.manager_mut().with_embedder(&sf_key, &emb_url, sf_model);
                    println!("  Embedding: SiliconFlow {} ✓", sf_model);
                }
            }
        }
    }
    let _ = engine.register_module(Box::new(memory_module));

    // 从 metabolism.toml 加载代谢配置
    let metabolism_cfg_path = profile_dir.join("modules").join("metabolism.toml");
    if let Ok(content) = std::fs::read_to_string(&metabolism_cfg_path) {
        let cfg = tremolite_memory::MetabolismConfig::from_toml(&content);
        let _ = engine.modules.with_module_mut("memory", |m| {
            if let Some(mm) = m
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::MemoryModule>())
            {
                mm.manager_mut().metabolism.config = cfg;
                // 设置配置路径，允许运行时读写
                mm.set_metabolism_config_path(metabolism_cfg_path.clone());
            }
        });
    }

    // 从 session.toml 读取会话配置
    let mut session_idle_timeout: u64 = 1800;
    let mut session_cleanup_timeout: u64 = 2592000;
    let session_cfg_path = profile_dir.join("modules").join("session.toml");
    if let Ok(content) = std::fs::read_to_string(&session_cfg_path) {
        if let Ok(toml_val) = content.parse::<toml::Value>() {
            if let Some(cfg) = toml_val.get("session") {
                session_idle_timeout = cfg.get("idle_timeout").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(1800);
                session_cleanup_timeout = cfg.get("cleanup_timeout").and_then(|v| v.as_integer()).map(|v| v as u64).unwrap_or(2592000);
            }
        }
    }
    let mut session_module = SessionModule::new(session_idle_timeout).with_session_path(profile_dir.clone());
    session_module.manager.set_cleanup_timeout(session_cleanup_timeout);
    let _ = engine.register_module(Box::new(session_module));
    let mut attn = AttentionModule::new();
    if let Some(ref cfg) = config {
        if let Some(ref emb_cfg) = cfg.embedding {
            if !emb_cfg.api_key.is_empty() {
                attn = attn.with_embedding_api(&emb_cfg.api_base, &emb_cfg.api_key, &emb_cfg.model);
            }
        }
    }
    // 从 modules/attention.toml 加载通道配置
    let attention_mod_path = profile_dir.join("modules").join("attention.toml");
    if let Ok(content) = std::fs::read_to_string(&attention_mod_path) {
        if let Ok(toml_val) = content.parse::<toml::Value>() {
            if let Some(attn_section) = toml_val.get("attention") {
                // 读取通道列表
                if let Some(channels_arr) = attn_section.get("channels").and_then(|v| v.as_array()) {
                    let channels: Vec<tremolite_attention::Channel> = channels_arr.iter().filter_map(|ch| {
                        let name = ch.get("name")?.as_str()?;
                        let label = ch.get("label").and_then(|v| v.as_str()).unwrap_or(name);
                        let window = ch.get("window")?.as_integer()? as usize;
                        let stride = ch.get("stride")?.as_integer()? as usize;
                        let max_blocks = ch.get("max_blocks")?.as_integer()? as usize;
                        let threshold = ch.get("threshold")?.as_float()?;
                        Some(tremolite_attention::Channel::new(name, label, window, stride, max_blocks, threshold))
                    }).collect();
                    if !channels.is_empty() {
                        attn = attn.with_channels(channels);
                    }
                }
                // 读取注入冷却设置
                if let Some(cooldown) = attn_section.get("inject_cooldown_rounds").and_then(|v| v.as_integer()) {
                    attn = attn.with_inject_cooldown(cooldown as u32);
                }
            }
        }
    }
    let _ = engine.register_module(Box::new(attn));

    // 注入 attention stats 路径——让每次扫描后持久化统计信息
    {
        let attn_stats_path = profile_dir.join("attention_stats.json");
        let attn_stats_str = attn_stats_path.to_string_lossy().to_string();
        let _ = engine.modules.with_module_mut("attention", |m| {
            if let Some(a) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::AttentionModule>())
            {
                a.set_stats_path(&attn_stats_str);
            }
        });
        let inject_log_path = profile_dir.join("attention_inject_log.txt");
        let inject_log_str = inject_log_path.to_string_lossy().to_string();
        let _ = engine.modules.with_module_mut("attention", |m| {
            if let Some(a) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::AttentionModule>())
            {
                a.set_inject_log_path(&inject_log_str);
            }
        });
        // 生成初始 stats 文件（如果不存在）
        if !attn_stats_path.exists() {
            let initial = serde_json::json!({
                "history_count": 0,
                "total_tokens_scanned": 0,
                "last_scan": null,
            });
            let _ = std::fs::write(&attn_stats_path, serde_json::to_string_pretty(&initial).unwrap());
        }
    }

    let _ = engine.register_module(Box::new(SkillModule::new(d.clone())));
    let _ = engine.register_module(Box::new(KanbanModule::new(d.clone())));
    let _ = engine.register_module(Box::new(ReflectionModule::new(5)));
    let _ = engine.register_module(Box::new(CompressModule::new()));
    let _ = engine.register_module(Box::new(DelegationModule::new()));
    let _ = engine.register_module(Box::new(ChannelsModule::new()));
    let _ = engine.register_module(Box::new(CronModule::new()));

    let mut user_module = UserModule::new();
    user_module.load_config(None, &username, &ai_name);
    // 从配置包加载模块配置（auto_mode / timed_mode / message_interval）
    user_module.load_module_config(&profile_dir);
    let _ = engine.register_module(Box::new(user_module));

    // 注入 cron_tasks.json 路径——让 CronModule 每 tick 从 API 写的文件加载
    {
        let cron_json = profile_dir.join("cron_tasks.json").to_string_lossy().to_string();
        let _ = engine.modules.with_module_mut("cron", |m| {
            if let Some(cm) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::CronModule>())
            {
                cm.set_json_path(&cron_json);
            }
        });
    }

    // MCP 模块——优先从配置包 modules/mcp.toml 加载（支持 stdio/HTTP/SSE），
    // 无配置包文件时回退到全局 config.toml 的 [mcp] 段（仅 HTTP URL）
    {
        use tremolite_mcp::{TransportConfig, McpServerConfig};
        let mcp_configs: Vec<McpServerConfig> = {
            let module_mcp_path = profile_dir.join("modules").join("mcp.toml");
            if module_mcp_path.exists() {
                // 从配置包加载——支持 transport/command/args 等完整字段
                let content = std::fs::read_to_string(&module_mcp_path).unwrap_or_default();
                let parsed: toml::Value = content.parse().unwrap_or(toml::Value::Table(toml::map::Map::new()));
                parsed.get("mcp")
                    .and_then(|m| m.get("servers"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter().filter_map(|entry| {
                            let name = entry.get("name")?.as_str()?.to_string();
                            let transport_str = entry.get("transport").and_then(|t| t.as_str()).unwrap_or("http");
                            let prefix = entry.get("prefix").and_then(|p| p.as_str()).unwrap_or("").to_string();
                            let timeout_secs = entry.get("timeout_secs").and_then(|t| t.as_integer()).unwrap_or(30) as u64;
                            let transport = match transport_str {
                                "stdio" => {
                                    let command = entry.get("command")?.as_str()?.to_string();
                                    let args: Vec<String> = entry.get("args")
                                        .and_then(|a| a.as_array())
                                        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                                        .unwrap_or_default();
                                    TransportConfig::Stdio { command, args }
                                }
                                "sse" => {
                                    let url = entry.get("url")?.as_str()?.to_string();
                                    TransportConfig::Sse { url }
                                }
                                _ => {
                                    let url = entry.get("url").or_else(|| entry.get("command"))
                                        .and_then(|v| v.as_str())?.to_string();
                                    TransportConfig::Http { url }
                                }
                            };
                            Some(McpServerConfig { name, transport, prefix, timeout_secs, env: std::collections::HashMap::new() })
                        }).collect()
                    })
                    .unwrap_or_default()
            } else {
                // 回退到全局 config.toml 的 [mcp] 段（仅 HTTP URL）
                config.as_ref()
                    .map(|c| c.mcp.servers.iter().map(|s| {
                        McpServerConfig {
                            name: s.name.clone(),
                            transport: TransportConfig::Http { url: s.url.clone() },
                            prefix: s.prefix.clone(),
                            timeout_secs: s.timeout_secs,
                            env: std::collections::HashMap::new(),
                        }
                    }).collect())
                    .unwrap_or_default()
            }
        };
        if mcp_configs.is_empty() {
            let _ = engine.register_module(Box::new(McpModule::new().with_cache_path(profile_dir.join("..").join("..").join("data").join("mcp_discovered.json"))));
        } else {
            tracing::info!("mcp: loading {} server(s) from profile module config", mcp_configs.len());
            let mcp_cache_path = profile_dir.join("..").join("..").join("data").join("mcp_discovered.json");
            let _ = engine.register_module(Box::new(McpModule::with_config(McpModule::new(), mcp_configs).with_cache_path(mcp_cache_path)));
        }
    }
    // Webhook 模块——外部事件监听与自动化流水线
    let _ = engine.register_module(Box::new(WebhookModule::new()));

    // 为技能系统注入 LLM 回调——启用自学习 + 蒸馏 + 三层流转
    {
        let providers = engine.providers.clone();
        let llm_fn: Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync> = Arc::new(move |prompt| {
            let provider = providers
                .get_default()
                .ok_or_else(|| "no default provider".to_string())?;
            let messages = vec![
                tremolite_llm::Message::system("你是一个技能蒸馏器。"),
                tremolite_llm::Message::user(prompt),
            ];
            let response = provider.chat(&messages, &[]).map_err(|e| e.to_string())?;
            Ok(response.content)
        });
        // 注入到 SkillModule——learn_cycle 内部自动调用 LLM 蒸馏
        let _ = engine.modules.with_module_mut("skill", |m| {
            if let Some(sm) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::SkillModule>())
            {
                sm.set_llm(llm_fn);
                println!("  Skill LLM injected ✓");
            }
        });
    }

    // 首次启动时运行学习循环——自动归域
    {
        let _ = engine.modules.with_module_mut("skill", |m| {
            if let Some(sm) = m.as_any_mut()
                .and_then(|a| a.downcast_mut::<tremolite_core::SkillModule>())
            {
                let stats = sm.learn_cycle();
                if stats.new_domains > 0 || stats.new_knowledge > 0 {
                    println!("  Initial learn cycle: {} domains, {} knowledge",
                        stats.new_domains, stats.new_knowledge);
                }
            }
        });
    }
    // 写出模块注册表供 dashboard API 动态读取
    {
        let home = std::env::var("HOME").unwrap_or_default();
        let reg_path = std::path::PathBuf::from(&home)
            .join(".tremolite").join("data").join("tremolite").join("modules_registry.json");
        let infos = engine.modules.handle().list_modules();
        if let Ok(json) = serde_json::to_string_pretty(&infos) {
            if let Some(parent) = reg_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&reg_path, &json);
        }
    }
    println!("  Modules registered ✓");

    // ── 6. 初始化定时任务 ────────────────────────
    if let Some(ref cfg) = config {
        if cfg.cron.enabled && !cfg.cron.jobs.is_empty() {
            println!("  Initializing cron scheduler...");
            for (key, job_cfg) in &cfg.cron.jobs {
                let name = job_cfg.name.clone().unwrap_or_else(|| key.clone());
                let schedule = match &job_cfg.schedule {
                    tremolite_config::CronScheduleConfig::EverySecs(s) => tremolite_cron::Schedule::EverySecs(*s),
                    tremolite_config::CronScheduleConfig::Daily { hour, minute } => tremolite_cron::Schedule::Daily { hour: *hour, minute: *minute },
                    tremolite_config::CronScheduleConfig::Once { delay_secs } => tremolite_cron::Schedule::Once { delay_secs: *delay_secs },
                    tremolite_config::CronScheduleConfig::CronExpr(e) => tremolite_cron::Schedule::CronExpr(e.clone()),
                };
                let _ = engine.modules.with_module_mut("cron", |m| {
                    if let Some(cm) = m.as_any_mut()
                        .and_then(|a| a.downcast_mut::<tremolite_core::CronModule>())
                    {
                        match &job_cfg.action {
                            tremolite_config::CronActionConfig::LlmPrompt { prompt } => {
                                cm.add_job(&name, schedule, prompt, "cron");
                            }
                            tremolite_config::CronActionConfig::Shell { command } => {
                                cm.add_shell_job(&name, schedule, command, "cron");
                            }
                        }
                    }
                });
                println!("  Cron job '{}' ✓", name);
            }
            println!("  Cron scheduler registered ✓");
        }
    }

    // ── 7. 广播 Startup 事件——通知所有模块完成初始化 ──
    use tremolite_core::Event;
    let startup_ctx = tremolite_core::EventContext::with_session(engine.modules.handle(), engine.session_id.clone());
    let _ = engine.modules.broadcast(&Event::Startup, &startup_ctx);

    // ── 8. 启动 ─────────────────────────────────
    match parsed.subcommand {
        cli::Subcommand::Run => {
            println!();
            println!("  Tremolite is alive. Waiting for your voice, Kamisama.");
            println!("  (enter /help for commands, /exit to quit)");
            println!();
            engine.run();
            println!("  Tremolite says goodbye. Until next time, Kamisama.");
        }
        cli::Subcommand::Tui => {
            match tui::run_tui(&mut engine) {
                Ok(()) => println!("\n  Tremolite says goodbye. Until next time, Kamisama."),
                Err(e) => eprintln!("\n  TUI Error: {}", e),
            }
        }
        cli::Subcommand::Daemon { .. } | cli::Subcommand::Dashboard { .. } => {
            let addr = format!("0.0.0.0:{port}");
            let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

            // 先进入 tokio 运行时上下文，让 initialize_channels 能拿到 Handle
            let _runtime_guard = rt.enter();

            let has_channels = config
                .as_ref()
                .map(|c| !c.channels.is_empty())
                .unwrap_or(false);

            // 创建 SessionScheduler——所有消息（包括 HTTP/WS）统一走调度器
            println!("  Creating session scheduler...");
            let (inbound_tx, outbound_rx, pending_results) = engine.create_scheduler();
            let chat_tx = inbound_tx.clone();
            let mut channel_registry: Option<Arc<tokio::sync::Mutex<ChannelRegistry>>> = None;

            if has_channels {
                if let Some(ref cfg) = config {
                    println!("  Initializing message channels...");
                    let registry = initialize_channels(&cfg.channels, &parsed.profile_name);
                    let mut channels_module = ChannelsModule::from_registry(registry);
                    let channel_names: Vec<String> = channels_module.list_channels();
                    if !channel_names.is_empty() {
                        println!("  Channels registered: {}", channel_names.join(", "));
                    }

                    // 桥接通道模块到调度器
                    println!("  Bridging channels to scheduler...");
                    channels_module.bridge_to_scheduler(inbound_tx, outbound_rx);
                    channel_registry = channels_module.channel_registry();
                    let _ = engine.register_module(Box::new(channels_module));
                }
            } else {
                println!("  No channels configured, running in API-only mode");
            }

            // 启动 HTTP 服务
            println!("  Starting HTTP daemon on http://{addr}");
            if let Err(e) = rt.block_on(run_server(chat_tx, pending_results, &addr, &parsed.profile_name, channel_registry, Some(engine.modules.clone()))) {
                eprintln!("  Server error: {e}");
            }
        }
        _ => unreachable!(),
    }
}

// ═══════════════════════════════════════════════════════
// 子命令处理器
// ═══════════════════════════════════════════════════════

fn handle_plan(action: &str, _args: &[String], config: Option<&Config>) {
    let data_dir = config
        .map(|c| PathBuf::from(&c.core.data_dir))
        .unwrap_or_else(|| PathBuf::from("./data/tremolite"));

    match action {
        "list" | "" => {
            let plans_dir = data_dir.join("plans");
            if let Ok(entries) = std::fs::read_dir(&plans_dir) {
                println!("Plans in {}:", plans_dir.display());
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".json") {
                        let short = name.trim_end_matches(".json");
                        let path = entry.path();
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            let summary = content.lines().next()
                                .map(|l| l.chars().take(80).collect::<String>())
                                .unwrap_or_default();
                            println!("  {}: {}", short, summary);
                        }
                    }
                }
            } else {
                println!("No plans directory found at {}", plans_dir.display());
            }
        }
        "dir" => {
            println!("Plans directory: {}", data_dir.join("plans").display());
        }
        other => {
            println!("Unknown plan action: '{}'. Try: list", other);
        }
    }
}

fn handle_tool(action: &str, _args: &[String]) {
    let tools = ToolsModule::new(None);

    match action {
        "list" | "" => {
            let names = tools.tool_names();
            println!("Built-in tools ({} total):", names.len());
            for name in &names {
                println!("  {}", name);
            }
        }
        other => {
            println!("Unknown tool action: '{}'. Try: list", other);
        }
    }
}

fn handle_skill(action: &str, _args: &[String], config: Option<&Config>) {
    let data_dir = config
        .map(|c| PathBuf::from(&c.core.data_dir))
        .unwrap_or_else(|| PathBuf::from("./data/tremolite"));

    match action {
        "list" | "" => {
            let skills_dir = data_dir.join("skills");
            if skills_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&skills_dir) {
                    println!("Skills in {}:", skills_dir.display());
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".md") {
                            println!("  {}", name);
                        }
                    }
                }
            } else {
                println!("Skills directory not found at {}", skills_dir.display());
                println!("Create skills with: mkdir -p {}", skills_dir.display());
            }
        }
        other => {
            println!("Unknown skill action: '{}'. Try: list", other);
        }
    }
}

fn handle_config(action: &str, _args: &[String], config: Option<&Config>) {
    match action {
        "show" | "" => {
            if let Some(cfg) = config {
            println!("{:#?}", cfg);
            } else {
                println!("No config loaded.");
            }
        }
        "path" => {
            let paths = [
                "./config.toml",
                "/etc/tremolite/config.toml",
                "~/.config/tremolite/config.toml",
            ];
            println!("Config search paths:");
            for p in &paths {
                println!("  {} {}", p, if std::path::Path::new(p).exists() { "✓" } else { "" });
            }
        }
        "export" => {
            println!("# tremolite config.toml — generated by `tremolite config export`");
            println!("# Copy this to config.toml and fill in your API keys.");
            println!();
            println!("[core]");
            println!("data_dir = \"./data/tremolite\"");
            println!();
            println!("[llm]");
            println!("default = \"openai\"");
            println!("[llm.providers.openai]");
            println!("type = \"openai\"");
            println!("api_key = \"${{OPENAI_API_KEY}}\"");
            println!("model = \"gpt-4o\"");
            println!("base_url = \"https://api.openai.com/v1\"");
        }
        other => {
            println!("Unknown config action: '{}'. Try: show, path, export", other);
        }
    }
}

fn handle_health(config: Option<&Config>) {
    let data_dir = config
        .map(|c| PathBuf::from(&c.core.data_dir))
        .unwrap_or_else(|| PathBuf::from("./data/tremolite"));

    println!("╔═══════════════════════════════════════╗");
    println!("║     Tremolite Health Check            ║");
    println!("╚═══════════════════════════════════════╝");
    println!();

    // 版本
    println!("  Version:    {}", VERSION);

    // 配置
    match config {
        Some(_) => println!("  Config:     ✓ loaded"),
        None => println!("  Config:     ✗ not loaded"),
    }

    // 数据目录
    let data_ok = data_dir.exists();
    println!("  Data dir:   {} {}", data_dir.display(), if data_ok { "✓" } else { "✗" });

    // 工具
    let tools = ToolsModule::new(None);
    println!("  Tools:      {} registered", tools.tool_count());

    // 技能目录
    let skills_dir = data_dir.join("skills");
    let skill_count = if skills_dir.exists() {
        std::fs::read_dir(&skills_dir).map(|e| e.count()).unwrap_or(0)
    } else {
        0
    };
    println!("  Skills:     {} files", skill_count);

    // 计划目录
    let plans_dir = data_dir.join("plans");
    let plan_count = if plans_dir.exists() {
        std::fs::read_dir(&plans_dir).map(|e| e.count()).unwrap_or(0)
    } else {
        0
    };
    println!("  Plans:      {} files", plan_count);
}

fn handle_session(action: &str, _args: &[String], _config: Option<&Config>) {
    match action {
        "list" | "" => {
            let sessions_dir = PathBuf::from("./data/tremolite/sessions");
            if sessions_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        let modified = entry
                            .metadata()
                            .and_then(|m| m.modified())
                            .map(|t| {
                                let secs = t.duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_secs())
                                    .unwrap_or(0);
                                chrono_format(secs)
                            })
                            .unwrap_or_else(|_| "?".into());
                        println!("  {} (last: {})", name, modified);
                    }
                }
            } else {
                println!("No sessions directory.");
            }
        }
        "dir" => {
            println!("Sessions directory: ./data/tremolite/sessions");
        }
        other => {
            println!("Unknown session action: '{}'. Try: list", other);
        }
    }
}

/// 简易时间格式化（替代 chrono crate）
fn chrono_format(unix_secs: u64) -> String {
    let secs_per_day = 86400u64;
    let days = unix_secs / secs_per_day;
    let time_secs = unix_secs % secs_per_day;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let secs = time_secs % 60;

    let mut y = 1970i64;
    let mut remaining_days = days as i64;
    loop {
        let year_days = if is_leap_year(y) { 366 } else { 365 };
        if remaining_days < year_days {
            break;
        }
        remaining_days -= year_days;
        y += 1;
    }

    let month_days: &[i64; 12] = if is_leap_year(y) { &LEAP_MONTH_DAYS } else { &MONTH_DAYS };
    let mut m = 0usize;
    let mut d = remaining_days;
    for (i, &md) in month_days.iter().enumerate() {
        if d < md {
            m = i + 1;
            break;
        }
        d -= md;
    }
    if m == 0 { m = 12; d = 31; }
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", y, m, d + 1, hours, minutes, secs)
}

const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const LEAP_MONTH_DAYS: [i64; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

// ─── Delegate 模式 ─────────────────────────────

/// `tremolite --delegate` 快速路径：读 stdin JSON → 执行 shell → 输出结果
fn handle_delegate() {
    use std::io::{self, BufRead};
    use tremolite_delegation::TaskContext;

    // 1. 读一行 stdin
    let mut line = String::new();
    let stdin = io::stdin();
    let mut locked = stdin.lock();
    if locked.read_line(&mut line).map_err(|e| e.to_string()).is_err() || line.trim().is_empty() {
        eprintln!("delegate: no input on stdin");
        std::process::exit(1);
    }

    // 2. 解析 TaskContext
    let ctx: TaskContext = match serde_json::from_str(line.trim()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("delegate: failed to parse TaskContext: {e}");
            std::process::exit(1);
        }
    };

    // 3. 执行 goal 作为 shell 命令
    let output = match std::process::Command::new("sh")
        .arg("-c")
        .arg(&ctx.goal)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("delegate: shell exec failed: {e}");
            std::process::exit(1);
        }
    };

    // 4. 输出第一行结果到 stdout
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.success() {
        // 取第一行
        let result_line = stdout.lines().next().unwrap_or("");
        println!("{}", result_line);
    } else {
        let msg = if stderr.is_empty() { &stdout } else { &stderr };
        let first = msg.lines().next().unwrap_or("unknown error");
        eprintln!("delegate: exit={} {}", output.status.code().unwrap_or(-1), first);
        std::process::exit(1);
    }
}

// ─── 模块管理 ─────────────────────────────────

/// 处理 `tremolite module <action> [args]` 命令
fn handle_module(action: String, args: Vec<String>, config: Option<&Config>) {
    use tremolite_packaging::{PackageReader, ModuleInstaller};

    let modules_dir = config
        .map(|c| PathBuf::from(&c.core.data_dir).join("modules"))
        .unwrap_or_else(|| PathBuf::from("./data/tremolite/modules"));

    let installer = ModuleInstaller::new(&modules_dir);

    match action.as_str() {
        "install" => {
            let path = args.first().map(|s| s.as_str()).unwrap_or("");
            if path.is_empty() {
                eprintln!("Usage: tremolite module install <path-to-.amod>");
                return;
            }
            println!("📦 Installing module from: {path}");
            match PackageReader::from_file(path) {
                Ok(pkg) => {
                    let m = pkg.manifest();
                    println!("   Module: {} v{}", m.module.name, m.module.version);
                    println!("   Provides: {}", m.module.declare.provides.join(", "));
                    match installer.install(&pkg) {
                        Ok(target) => println!("✅ Installed to: {}", target.display()),
                        Err(e) => eprintln!("❌ Install failed: {e}"),
                    }
                }
                Err(e) => eprintln!("❌ Failed to read package: {e}"),
            }
        }
        "uninstall" => {
            let module_id = args.first().map(|s| s.as_str()).unwrap_or("");
            if module_id.is_empty() {
                eprintln!("Usage: tremolite module uninstall <module-id>");
                return;
            }
            match installer.uninstall(module_id) {
                Ok(()) => println!("✅ Uninstalled module: {module_id}"),
                Err(e) => eprintln!("❌ {e}"),
            }
        }
        "list" | "ls" => {
            // 所有内建模块——透闪石出厂预装的小伙伴们
            let builtin_modules: Vec<(&str, &str, &str, Vec<&str>)> = vec![
                ("emotion",    "情绪引擎",       "0.3.0", vec!["emotion.detect", "emotion.style", "emotion.composite"]),
                ("tools",      "系统工具",       "0.3.0", vec!["tool.file_read", "tool.file_write", "tool.shell"]),
                ("memory",     "五层记忆",       "0.3.0", vec!["memory.store", "memory.recall", "memory.search"]),
                ("session",    "会话管理器",     "0.3.0", vec!["session.manage", "session.peek", "session.share"]),
                ("attention",  "多尺度注意力",   "0.3.0", vec!["attention.scan", "attention.summarize"]),
                ("skill",      "技能系统",       "0.3.0", vec!["skill.learn", "skill.practice", "skill.forget"]),
                ("board",      "看板",           "0.3.0", vec!["plan.create", "plan.track", "plan.advance"]),
                ("delegation", "任务委派",       "0.3.0", vec!["delegate.task", "delegate.session"]),
                ("cron",       "定时任务",       "0.3.0", vec!["cron.schedule", "cron.execute"]),
                ("mcp",        "MCP 客户端",     "0.3.0", vec!["mcp.discover", "mcp.call"]),
                ("webhook",    "Webhook 订阅",   "0.3.0", vec!["webhook.listen", "webhook.route"]),
                ("dashboard",  "仪表盘",         "0.3.0", vec!["dashboard.serve"]),
                ("reflection", "反思引擎",       "0.3.0", vec!["reflection.dialectic", "reflection.inject"]),
                ("compress",   "上下文压缩引擎", "0.3.0", vec!["compress.strategy", "compress.execute"]),
                ("distiller",  "技能蒸馏器",     "0.3.0", vec!["skill.distill"]),
            ];

            // 已安装的 .amod 模块
            let installed = match installer.list_installed() {
                Ok(modules) => modules,
                Err(e) => {
                    eprintln!("❌ Failed to list installed packages: {e}");
                    vec![]
                }
            };

            let total = builtin_modules.len() + installed.len();
            println!("📦 透闪石模块清单（共 {} 个）", total);
            println!();

            for (id, name, ver, provides) in &builtin_modules {
                println!("  {} v{}", name, ver);
                println!("    id:       {}", id);
                println!("    provides: {}", provides.join(", "));
                println!();
            }

            for m in &installed {
                println!("  {} v{}", m.name, m.version);
                println!("    id:        {}", m.id);
                println!("    language:  {}", m.language);
                println!("    provides:  {}", m.provides.join(", "));
                println!("    path:      {}", m.path.display());
                println!();
            }

            if installed.is_empty() {
                println!("  （没有额外安装的 .amod 包，上面这些都是出厂预装的喔~）");
                println!();
            }
        }
        "info" => {
            let module_id = args.first().map(|s| s.as_str()).unwrap_or("");
            if module_id.is_empty() {
                eprintln!("Usage: tremolite module info <module-id>");
                return;
            }
            match installer.list_installed() {
                Ok(modules) => {
                    if let Some(m) = modules.iter().find(|m| m.id == module_id) {
                        println!("📦 Module: {}", m.name);
                        println!("  id:        {}", m.id);
                        println!("  version:   {}", m.version);
                        println!("  language:  {}", m.language);
                        println!("  entry:     {}", m.entry);
                        println!("  provides:  {}", m.provides.join(", "));
                        println!("  path:      {}", m.path.display());
                    } else {
                        eprintln!("❌ Module '{module_id}' not installed.");
                    }
                }
                Err(e) => eprintln!("❌ {e}"),
            }
        }
        _ => {
            eprintln!("Unknown module action: {action}");
            eprintln!("Available: install, uninstall, list, info");
        }
    }
}
