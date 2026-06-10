use std::collections::HashMap;

/// 透闪石 CLI 子命令定义
#[derive(Debug)]
pub enum Subcommand {
    /// 交互式会话（默认）
    Run,
    /// Daemon 模式（HTTP API 服务）
    Daemon {
        port: u16,
        dashboard_port: u16,
    },
    /// TUI 模式
    Tui,
    /// Dashboard 模式
    Dashboard {
        port: u16,
        dashboard_port: u16,
    },
    /// 查看/管理计划书
    Plan {
        action: String,
        args: Vec<String>,
    },
    /// 查看/管理工具
    Tool {
        action: String,
        args: Vec<String>,
    },
    /// 查看/管理技能
    Skill {
        action: String,
        args: Vec<String>,
    },
    /// 配置管理
    Config {
        action: String,
        args: Vec<String>,
    },
    /// 健康检查
    Health,
    /// 会话管理
    Session {
        action: String,
        args: Vec<String>,
    },
    /// 版本信息
    Version,
    /// 帮助
    Help,
}

#[derive(Debug)]
pub struct ParsedCommand {
    pub subcommand: Subcommand,
    pub config_path: Option<String>,
    pub log_level: String,
    pub session_id: String,
    pub gateway_url: String,
}

/// 显示帮助文本
pub fn print_help() {
    println!("╔══════════════════════════════════════════╗");
    println!("║     透闪石 Tremolite v{}                 ║", crate::VERSION);
    println!("║     The autonomous AI agent framework    ║");
    println!("╚══════════════════════════════════════════╝");
    println!();
    println!("USAGE:");
    println!("  tremolite [OPTIONS] <COMMAND> [ARGS]");
    println!();
    println!("COMMANDS:");
    println!("  run          启动交互式会话（默认）");
    println!("  daemon       启动 HTTP API 服务模式");
    println!("  tui          启动终端 UI 模式");
    println!("  dashboard    启动 Web Dashboard");
    println!("  plan         查看/管理计划书");
    println!("  tool         查看/管理工具");
    println!("  skill        查看/管理技能");
    println!("  config       查看/导出配置");
    println!("  health       健康检查");
    println!("  session      管理会话");
    println!("  version      版本信息");
    println!("  help         显示此帮助");
    println!();
    println!("GLOBAL OPTIONS:");
    println!("  -c, --config <PATH>       指定配置文件路径");
    println!("  -l, --log-level <LEVEL>   日志级别 (trace/debug/info/warn/error)");
    println!("      --session <ID>        指定会话 ID");
    println!("      --gateway-url <URL>   Gateway 地址 (默认 http://localhost:8080)");
    println!();
    println!("DAEMON OPTIONS:");
    println!("  --port <PORT>             监听端口 (默认 721)");
    println!("  --dashboard-port <PORT>   Dashboard 端口 (默认 9090)");
    println!();
    println!("EXAMPLES:");
    println!("  tremolite run --session work");
    println!("  tremolite daemon --port 721");
    println!("  tremolite plan list");
    println!("  tremolite tool list");
    println!("  tremolite health");
}

/// 解析命令行参数为子命令
pub fn parse_args() -> ParsedCommand {
    let args: Vec<String> = std::env::args().collect();

    // 全局选项
    let config_path = parse_opt_string(&args, &["-c", "--config"]);
    let log_level = parse_opt_string(&args, &["-l", "--log-level"]).unwrap_or_else(|| "info".into());
    let session_id = parse_opt_string(&args, &["--session"]).unwrap_or_default();
    let gateway_url = parse_opt_string(&args, &["--gateway-url"]).unwrap_or_else(|| "http://localhost:8080".into());
    let port = parse_opt_uint(&args, &["--port"]).unwrap_or(721u16);
    let dashboard_port = parse_opt_uint(&args, &["--dashboard-port"]).unwrap_or(9090u16);

    // 找第一个非 flag 的参数作为子命令
    let subcmd_name = args.iter()
        .skip(1) // skip binary name
        .find(|a| !a.starts_with('-'))
        .map(|s| s.as_str())
        .unwrap_or("run");

    let subcommand = match subcmd_name {
        "run" => Subcommand::Run,
        "daemon" => Subcommand::Daemon { port, dashboard_port },
        "tui" => Subcommand::Tui,
        "dashboard" => Subcommand::Dashboard { port, dashboard_port },
        "plan" => parse_double_subcmd(&args, "plan", |a, r| Subcommand::Plan { action: a, args: r }),
        "tool" => parse_double_subcmd(&args, "tool", |a, r| Subcommand::Tool { action: a, args: r }),
        "skill" => parse_double_subcmd(&args, "skill", |a, r| Subcommand::Skill { action: a, args: r }),
        "config" => parse_double_subcmd(&args, "config", |a, r| Subcommand::Config { action: a, args: r }),
        "health" => Subcommand::Health,
        "session" => parse_double_subcmd(&args, "session", |a, r| Subcommand::Session { action: a, args: r }),
        "version" | "-v" | "--version" => Subcommand::Version,
        "help" | "-h" | "--help" => Subcommand::Help,
        // 向后兼容：老式 flag 检测
        _ => {
            if args.iter().any(|a| a == "--daemon") {
                Subcommand::Daemon { port, dashboard_port }
            } else if args.iter().any(|a| a == "--tui") {
                Subcommand::Tui
            } else if args.iter().any(|a| a == "--dashboard") {
                Subcommand::Dashboard { port, dashboard_port }
            } else {
                Subcommand::Run
            }
        }
    };

    ParsedCommand { subcommand, config_path, log_level, session_id, gateway_url }
}

/// 解析子命令的子动作（如 `tremolite plan list` 中的 "list"）
fn parse_double_subcmd(args: &[String], cmd_name: &str, factory: fn(String, Vec<String>) -> Subcommand) -> Subcommand {
    let mut rest = Vec::new();
    let mut found = false;
    let mut action = String::new();
    for arg in args {
        if !found && arg == cmd_name {
            found = true;
            continue;
        }
        if found && !arg.starts_with('-') {
            if action.is_empty() {
                action = arg.clone();
            } else {
                rest.push(arg.clone());
            }
        }
    }
    factory(action, rest)
}

fn parse_opt_string(args: &[String], names: &[&str]) -> Option<String> {
    for (i, arg) in args.iter().enumerate() {
        if names.contains(&arg.as_str()) {
            return args.get(i + 1).cloned();
        }
    }
    None
}

fn parse_opt_uint<T: std::str::FromStr>(args: &[String], names: &[&str]) -> Option<T> {
    for (i, arg) in args.iter().enumerate() {
        if names.contains(&arg.as_str()) {
            if let Some(val) = args.get(i + 1) {
                if let Ok(n) = val.parse() {
                    return Some(n);
                }
            }
        }
    }
    None
}
