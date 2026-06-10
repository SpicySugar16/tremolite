use std::io;
use std::time::{SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

use tremolite_core::TremoliteEngine;
// ToolCallLoop, ToolExecutor 通过 engine.process_with_llm 走

/// 获取当前时间的 HH:MM:SS 格式字符串
fn now_str() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // UTC+8
    let secs = d + 8 * 3600;
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// 透闪石 TUI 聊天界面 v2
pub fn run_tui(engine: &mut TremoliteEngine) -> Result<(), String> {
    enable_raw_mode().map_err(|e| format!("raw mode: {}", e))?;
    let mut stdout = io::stdout();
    stdout
        .execute(EnterAlternateScreen)
        .map_err(|e| format!("alternate screen: {}", e))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| format!("terminal: {}", e))?;
    let _ = terminal.clear();

    // ── 状态 ────────────────────────────────────────
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut input = String::new();
    let mut scroll_offset: usize = 0;
    let mut should_quit = false;
    let mut thinking = false;
    let mut input_history: Vec<String> = Vec::new();
    let mut history_index: usize = 0;

    let provider_name = engine
        .providers
        .get_default()
        .map(|p| p.name().to_string())
        .unwrap_or_else(|| "none".into());

    // 欢迎消息
    messages.push(ChatMessage {
        sender: "系统".into(),
        content: "✦ Tremolite TUI ✦".into(),
        style: MessageStyle::System,
        timestamp: now_str(),
    });

    // ── 主循环 ──────────────────────────────────────
    while !should_quit {
        let emotion = engine.emotion_display();

        terminal
            .draw(|f| {
                let size = f.area();
                draw_tui(
                    f,
                    size,
                    &messages,
                    &input,
                    scroll_offset,
                    thinking,
                    &provider_name,
                    &emotion,
                );
            })
            .map_err(|e| format!("draw: {}", e))?;

        if !event::poll(std::time::Duration::from_millis(100))
            .map_err(|e| format!("poll: {}", e))?
        {
            continue;
        }

        let evt = event::read().map_err(|e| format!("read: {}", e))?;

        match evt {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                match key.code {
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        should_quit = true;
                    }
                    KeyCode::Enter if !thinking => {
                        if !input.trim().is_empty() {
                            let raw = input.trim().to_string();
                            input.clear();

                            let trimmed = raw.trim().to_string();
                            if !trimmed.is_empty() {
                                input_history.push(trimmed.clone());
                                history_index = input_history.len();
                            }

                            match trimmed.as_str() {
                                "/exit" | "/quit" => {
                                    messages.push(ChatMessage {
                                        sender: "系统".into(),
                                        content: "Goodbye.".into(),
                                        style: MessageStyle::System,
                                        timestamp: now_str(),
                                    });
                                    should_quit = true;
                                }
                                "/help" => {
                                    messages.push(ChatMessage {
                                        sender: "系统".into(),
                                        content: format!(
                                            "命令列表:\n  /exit /quit  — 退出\n  /help       — 显示帮助\n  /clear      — 清屏\n  /emotion    — 显示当前情绪\n\n状态:\n  提供者: {}\n  情绪: {}",
                                            provider_name, emotion
                                        ),
                                        style: MessageStyle::System,
                                        timestamp: now_str(),
                                    });
                                }
                                "/clear" => {
                                    messages.clear();
                                    scroll_offset = 0;
                                }
                                "/emotion" => {
                                    messages.push(ChatMessage {
                                        sender: "系统".into(),
                                        content: format!(
                                            "当前情绪:\n  复合: {}",
                                            emotion
                                        ),
                                        style: MessageStyle::Info,
                                        timestamp: now_str(),
                                    });
                                }
                                _ => {
                                    // 用户消息
                                    messages.push(ChatMessage {
                                        sender: "You".into(),
                                        content: trimmed.clone(),
                                        style: MessageStyle::User,
                                        timestamp: now_str(),
                                    });

                                    // 先显示思考中
                                    thinking = true;
                                    let think_idx = messages.len();
                                    messages.push(ChatMessage {
                                        sender: "Agent".into(),
                                        content: "‖ thinking... ‖".into(),
                                        style: MessageStyle::Thinking,
                                        timestamp: now_str(),
                                    });

                                    // 刷新显示"思考中"
                                    let _ = terminal.draw(|f| {
                                        let size = f.area();
                                        draw_tui(
                                            f,
                                            size,
                                            &messages,
                                            &input,
                                            scroll_offset,
                                            thinking,
                                            &provider_name,
                                            &emotion,
                                        );
                                    });

                                    // 处理（阻塞LLM调用）
                                    let response = process_tui_message(engine, &trimmed);

                                    // 替换为实际回复
                                    messages[think_idx] = ChatMessage {
                                        sender: "Agent".into(),
                                        content: response,
                                        style: MessageStyle::Assistant,
                                        timestamp: now_str(),
                                    };
                                    thinking = false;
                                    scroll_offset = 0;
                                }
                            }
                        }
                    }
                    KeyCode::Up if !thinking && !input_history.is_empty() => {
                        if history_index > 0 {
                            history_index -= 1;
                            input = input_history[history_index].clone();
                        }
                    }
                    KeyCode::Down if !thinking && !input_history.is_empty() => {
                        if history_index < input_history.len() - 1 {
                            history_index += 1;
                            input = input_history[history_index].clone();
                        } else {
                            history_index = input_history.len();
                            input.clear();
                        }
                    }
                    KeyCode::Char(c) if !thinking => {
                        input.push(c);
                    }
                    KeyCode::Backspace if !thinking => {
                        input.pop();
                    }
                    KeyCode::Esc => {
                        should_quit = true;
                    }
                    KeyCode::PageUp => {
                        scroll_offset += 5;
                    }
                    KeyCode::PageDown => {
                        scroll_offset = scroll_offset.saturating_sub(5);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    disable_raw_mode().map_err(|e| format!("raw mode: {}", e))?;
    let mut stdout = io::stdout();
    stdout
        .execute(LeaveAlternateScreen)
        .map_err(|e| format!("leave screen: {}", e))?;
    Ok(())
}

// ─── 消息类型 ────────────────────────────────────

struct ChatMessage {
    sender: String,
    content: String,
    style: MessageStyle,
    timestamp: String,
}

enum MessageStyle {
    User,
    Assistant,
    Thinking,
    System,
    Info,
}

// ─── 布局绘制 ────────────────────────────────────

fn draw_tui(
    f: &mut ratatui::Frame,
    area: Rect,
    msgs: &[ChatMessage],
    input: &str,
    scroll: usize,
    thinking: bool,
    provider: &str,
    emotion: &str,
) {
    let jade = Color::Rgb(122, 200, 154); // 透辉石·水种玉
    let _jade_light = Color::Rgb(160, 218, 180);
    let jade_dark = Color::Rgb(80, 160, 110);
    let _jade_bg = Color::Rgb(30, 50, 38);

    // 上中下三块：消息区 | 状态栏 | 输入区
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // 消息区
            Constraint::Length(1), // 状态栏
            Constraint::Length(3), // 输入区
        ])
        .split(area);

    // ── 状态栏 ──────────────────────────────
    let status_text = if thinking {
        format!(
            " ◆ {} ◆ {} ◆ thinking...",
            provider, emotion
        )
    } else {
        format!(
            " ◇ {} ◆ {}",
            provider, emotion
        )
    };
    let status_paragraph = Paragraph::new(Line::from(Span::styled(
        &status_text,
        Style::default()
            .fg(Color::Rgb(180, 190, 200))
            .bg(jade_dark),
    )));
    f.render_widget(status_paragraph, chunks[1]);

    // ── 消息区 ──────────────────────────────
    let msg_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(jade))
        .title(Line::from(Span::styled(
            " 透闪石 Tremolite ",
            Style::default().fg(jade).add_modifier(Modifier::BOLD),
        )))
        .title_alignment(Alignment::Left);

    let inner = msg_block.inner(chunks[0]);

    let items: Vec<ListItem> = msgs
        .iter()
        .rev()
        .skip(scroll)
        .map(|msg| {
            let (prefix, msg_color) = match msg.style {
                MessageStyle::User => ("◈ You", jade),
                MessageStyle::Assistant => ("◇ Agent", Color::Rgb(100, 200, 150)),
                MessageStyle::Thinking => ("◇ Agent", Color::Rgb(100, 180, 120)),
                MessageStyle::System => ("◆ 系统", Color::Rgb(200, 200, 100)),
                MessageStyle::Info => ("● 信息", Color::Rgb(100, 180, 255)),
            };

            let mut lines = vec![Line::from(Span::styled(
                format!(" {} {} ", prefix, msg.timestamp),
                Style::default().fg(msg_color).add_modifier(Modifier::BOLD),
            ))];

            let content_color = match msg.style {
                MessageStyle::Thinking => Color::Rgb(100, 160, 120),
                _ => Color::Rgb(200, 210, 220),
            };

            for line_str in msg.content.lines() {
                lines.push(Line::from(Span::styled(
                    line_str,
                    Style::default().fg(content_color),
                )));
            }
            lines.push(Line::from(""));

            ListItem::new(lines)
        })
        .collect();

    let msg_list = List::new(items).block(msg_block);
    f.render_widget(msg_list, inner);

    // ── 输入区 ──────────────────────────────
    let input_title = if thinking { " processing… " } else { " input " };

    let input_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(jade))
        .title(Line::from(Span::styled(
            input_title,
            Style::default().fg(if thinking {
                Color::Rgb(100, 160, 120)
            } else {
                jade
            }),
        )))
        .title_alignment(Alignment::Left);

    let input_text = Paragraph::new(input)
        .style(Style::default().fg(Color::Rgb(230, 230, 240)))
        .block(input_block)
        .wrap(Wrap { trim: true });

    f.render_widget(input_text, chunks[2]);

    // 光标只在非思考时显示
    if !thinking {
        let input_width = input.width() as u16;
        f.set_cursor_position((
            chunks[2].x + 1 + input_width.min(chunks[2].width.saturating_sub(3)),
            chunks[2].y + 1,
        ));
    }
}

// ─── 消息处理 ────────────────────────────────────

/// 处理 TUI 消息——走引擎的 PromptBuilder + collect_prompt_segments
fn process_tui_message(engine: &mut TremoliteEngine, input: &str) -> String {
    let channel = "tui";

    // 广播 OnMessage —— 模块处理情绪检测、记忆写入、注意力扫描
    let ctx = tremolite_core::EventContext::with_session(engine.modules.handle(), engine.session_id.clone());
    let _ = engine.modules.broadcast(
        &tremolite_core::Event::OnMessage {
            input: input.to_string(),
            channel: channel.to_string(),
        },
        &ctx,
    );

    // 直接走引擎：构建 prompt、调 LLM（CLI 模式无调度器）
    use tremolite_llm::{Message, PromptContext, ToolCallLoop};

    // 从所有已注册模块收集 prompt 片段
    let module_segments: Vec<String> = engine.modules.collect_prompt_segments()
        .into_iter()
        .map(|(_id, segment)| segment)
        .collect();

    // 构建完整 system prompt
    let mut prompt_parts = vec![engine.base_soul.clone()];
    prompt_parts.extend(module_segments);
    let full_prompt = prompt_parts.join("\n\n");
    engine.prompt_builder.set_system_prompt(&full_prompt);

    // 工具列表
    let all_tools: Vec<String> = engine.tool_executor.list_tools()
        .iter().map(|t| t.function.name.clone()).collect();
    let available_tools: Vec<String> = all_tools.into_iter().filter(|name| {
        name == "use_tool" || engine.modules.with_module("skill", |m| {
            m.as_any()
                .and_then(|any| any.downcast_ref::<tremolite_core::modules::skill::SkillModule>())
                .map(|sm| sm.engine().get_success_rate(name))
                .unwrap_or(0.5)
        }).unwrap_or(0.5) >= 0.3
    }).collect();

    // 获取最近对话历史
    let history: Vec<Message> = engine.modules.with_module("memory", |m| {
        m.as_any()
            .and_then(|any| any.downcast_ref::<tremolite_core::modules::memory::MemoryModule>())
            .map(|mm| {
                mm.recent_entries(&engine.session_id, 20).iter().filter_map(|entry| {
                    let c = &entry.content;
                    if let Some(user_msg) = c.strip_prefix("kamisama: ") {
                        Some(Message::user(user_msg))
                    } else if let Some(assistant_msg) = c.strip_prefix("葵: ") {
                        Some(Message::assistant(assistant_msg))
                    } else { None }
                }).collect::<Vec<Message>>()
            })
            .unwrap_or_default()
    }).unwrap_or_default();

    let prompt_ctx = PromptContext {
        user_input: input.to_string(),
        conversation_history: history,
        available_tools,
    };
    let messages = engine.prompt_builder.build(&prompt_ctx);

    let reply = if let Some(provider) = engine.providers.get_default() {
        let tool_loop = ToolCallLoop::new();
        let executor: &dyn tremolite_llm::ToolExecutor = &*engine.tool_executor;
        match tool_loop.run(provider, &messages, executor) {
            Ok(result) => {
                for record in &result.call_history {
                    let _ = engine.modules.with_module_mut("skill", |m| {
                        if let Some(sm) = m.as_any_mut()
                            .and_then(|any| any.downcast_mut::<tremolite_core::modules::skill::SkillModule>())
                        {
                            sm.engine_mut().practice("use_tool", record.success, &record.tool_name);
                        }
                    });
                }
                result.content
            }
            Err(e) => format!("[LLM Error: {e}]"),
        }
    } else {
        // 无 LLM provider 时的 fallback
        let emotion = engine.modules.with_module("emotion", |m| {
            m.as_any()
                .and_then(|any| any.downcast_ref::<tremolite_core::modules::emotion::EmotionModule>())
                .map(|em| em.composite_emotion())
                .unwrap_or_default()
        }).unwrap_or_default();
        match emotion.as_str() {
            "爱" | "快乐" | "欣喜" => "噜噜……神大人说的呢，葵听到了喔~".into(),
            "悲伤" | "焦虑" => "呜……神大人说这样的话，葵有点担心呢……".into(),
            "愤怒" | "不满" => "哼~神大人这样说葵可不高兴呢……".into(),
            _ => "噜噜……神大人说的葵听到了，葵正在努力理解喔~".into(),
        }
    };

    // 广播 OnResponse —— 模块处理记忆存储、代谢、学习洞察
    let _ = engine.modules.broadcast(
        &tremolite_core::Event::OnResponse {
            response: reply.clone(),
        },
        &ctx,
    );

    reply
}
