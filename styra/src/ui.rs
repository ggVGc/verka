//! Terminal rendering of [`App`] with ratatui.
//!
//! Three stacked regions: the event list (each entry a summary line that grows
//! inline when expanded), the message box, and a one-line status/help footer.
//! Rendering is a pure function of `App`; all state lives in [`crate::app`].

use crate::app::{App, Entry, Focus, Status, View};
use crate::event::{DetailBlock, AgentEvent};
use crate::session::{Direction as WireDirection, LogLevel};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// Cap on detail lines shown for one expanded entry, so a single noisy command
/// cannot bury the rest of the session.
const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";

/// Color coding for the status dot, so running vs. waiting for input reads at
/// a glance instead of requiring the operator to read the label text.
fn status_color(status: &Status) -> Color {
    match status {
        Status::Running => Color::Green,
        Status::Waiting => Color::Yellow,
        Status::Stopped => Color::DarkGray,
        Status::Ended { error: Some(_), .. } => Color::Red,
        Status::Ended { .. } => Color::DarkGray,
    }
}

/// Build a block title of the form " styra · profile · ● status[ · suffix] ".
fn title_line(profile: &str, status: &Status, suffix: Option<&str>) -> Line<'static> {
    let color = status_color(status);
    let mut spans = vec![
        Span::raw(format!(" styra · {profile} · ")),
        Span::styled("● ", Style::default().fg(color)),
        Span::styled(status.label(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ];
    spans.push(match suffix {
        Some(suffix) => Span::raw(format!(" · {suffix} ")),
        None => Span::raw(" "),
    });
    Line::from(spans)
}

pub fn render(frame: &mut Frame, app: &App) {
    let input_height = input_area_height(app);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(frame.area());

    match app.view {
        View::Events => render_list(frame, app, chunks[0]),
        View::Raw => render_raw(frame, app, chunks[0]),
        View::Log => render_log(frame, app, chunks[0]),
    }
    render_input(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
}

fn render_log(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.profile_name, &app.status, Some("log")));

    if app.log.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no log entries yet",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let lines: Vec<Line<'static>> = app.log.iter().map(log_line).collect();
    let viewport = area.height.saturating_sub(2) as usize;
    let max_start = lines.len().saturating_sub(viewport);
    let start = max_start.saturating_sub(app.log_scroll_back as usize) as u16;
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
}

fn log_line(entry: &crate::session::LogEntry) -> Line<'static> {
    let (label, color) = match entry.level {
        LogLevel::Info => ("info ", Color::Gray),
        LogLevel::Warn => ("warn ", Color::Yellow),
        LogLevel::Error => ("error", Color::Red),
    };
    Line::from(vec![
        Span::styled(format!("{label} "), Style::default().fg(color).add_modifier(Modifier::BOLD)),
        Span::styled(entry.message.clone(), Style::default().fg(Color::White)),
    ])
}

fn render_raw(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.profile_name, &app.status, Some("raw")));

    if app.raw.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no wire traffic yet",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let lines: Vec<Line<'static>> = app.raw.iter().map(raw_line).collect();
    // Anchor to the bottom, offset upward by how far the operator scrolled.
    let viewport = area.height.saturating_sub(2) as usize;
    let max_start = lines.len().saturating_sub(viewport);
    let start = max_start.saturating_sub(app.raw_scroll_back as usize) as u16;
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
}

fn raw_line(line: &crate::session::RawLine) -> Line<'static> {
    let (marker, color) = match line.direction {
        WireDirection::ToAgent => ("» ", Color::Cyan),
        WireDirection::FromAgent => ("« ", Color::Green),
    };
    Line::from(vec![
        Span::styled(marker, Style::default().fg(color)),
        Span::styled(line.text.clone(), Style::default().fg(Color::White)),
    ])
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let usage = app
        .latest_usage
        .as_ref()
        .map(|u| {
            format!(
                " in {} · out {} · cached {} ",
                u.input_tokens, u.output_tokens, u.cached_input_tokens
            )
        })
        .unwrap_or_default();
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.profile_name, &app.status, None))
        .title_bottom(Line::from(usage).right_aligned());

    if app.entries.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  waiting for the agent — press i to send a message",
            Style::default().fg(Color::Gray),
        )]))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let visible: Vec<(usize, &Entry)> = app
        .entries
        .iter()
        .enumerate()
        .filter(|(idx, _)| app.is_visible(*idx))
        .collect();

    if visible.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  all entries hidden — press m to show minor events",
            Style::default().fg(Color::Gray),
        )]))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let width = area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = visible
        .iter()
        .map(|(_, entry)| entry_item(entry, width))
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    let position = visible
        .iter()
        .position(|(idx, _)| *idx == app.selected)
        .or_else(|| visible.iter().rposition(|(idx, _)| *idx < app.selected));
    state.select(position);
    frame.render_stateful_widget(list, area, &mut state);
}

fn entry_item(entry: &Entry, width: usize) -> ListItem<'static> {
    let mut lines = vec![summary_line(entry)];
    if entry.expanded {
        lines.extend(detail_lines(&entry.event));
    }
    let wrapped: Vec<Line<'static>> = lines
        .into_iter()
        .flat_map(|line| wrap_line(line, width))
        .collect();
    ListItem::new(wrapped)
}

/// Word-wrap one logical line to `width` columns, preserving each span's
/// style across the break. `List` does not wrap on its own, so long lines
/// would otherwise be clipped at the right edge instead of continuing below.
fn wrap_line(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }

    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for span in line.spans {
        let style = span.style;
        for token in split_keep_whitespace(&span.content) {
            let token_width = token.chars().count();

            if token == " " {
                if current_width + token_width > width {
                    if !current.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                    }
                    continue;
                }
                current.push(Span::styled(token, style));
                current_width += token_width;
                continue;
            }

            if token_width > width {
                // A single token longer than the line: hard-split it.
                let mut remaining = token.as_str();
                while !remaining.is_empty() {
                    if current_width >= width {
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                    }
                    let take = width - current_width;
                    let split_at = remaining
                        .char_indices()
                        .nth(take)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    let (chunk, rest) = remaining.split_at(split_at);
                    current.push(Span::styled(chunk.to_owned(), style));
                    current_width += chunk.chars().count();
                    remaining = rest;
                }
                continue;
            }

            if current_width + token_width > width && !current.is_empty() {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
            current.push(Span::styled(token, style));
            current_width += token_width;
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

/// Split into words and single-space tokens, so a wrap can drop a leading
/// space on the next line without losing the boundary information.
fn split_keep_whitespace(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for ch in s.chars() {
        if ch == ' ' {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            tokens.push(" ".to_owned());
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}

fn summary_line(entry: &Entry) -> Line<'static> {
    let marker = if entry.expanded { "▾" } else { "▸" };
    let tag = entry.event.tag();
    Line::from(vec![
        Span::raw(format!("{marker} ")),
        Span::styled(
            format!("{tag:<8} "),
            Style::default().fg(tag_color(tag)).add_modifier(Modifier::BOLD),
        ),
        Span::styled(entry.event.summary(), Style::default().fg(Color::White)),
    ])
}

fn detail_lines(event: &AgentEvent) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in event.detail() {
        match block {
            DetailBlock::Text(text) => {
                for line in text.lines() {
                    lines.push(Line::from(vec![Span::styled(
                        format!("{DETAIL_INDENT}{line}"),
                        Style::default().fg(Color::White),
                    )]));
                }
            }
            DetailBlock::Code { text, .. } => {
                for line in text.lines() {
                    lines.push(Line::from(vec![Span::styled(
                        format!("{DETAIL_INDENT}{line}"),
                        Style::default().fg(Color::White),
                    )]));
                }
            }
        }
    }
    // The detail body's first line is invariably a restatement of what the
    // summary line above it already shows (the command, the message's first
    // line, ...); drop it so expanding an entry doesn't just echo itself.
    if !lines.is_empty() {
        lines.remove(0);
    }
    if lines.len() > MAX_DETAIL_LINES {
        let hidden = lines.len() - MAX_DETAIL_LINES;
        lines.truncate(MAX_DETAIL_LINES);
        lines.push(Line::from(Span::styled(
            format!("{DETAIL_INDENT}… {hidden} more lines"),
            Style::default().fg(Color::Gray),
        )));
    }
    lines
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Input;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title = if app.can_send() {
        " message "
    } else {
        " message (session ended) "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(area);
    let paragraph = Paragraph::new(input_text(app)).block(block);
    frame.render_widget(paragraph, area);

    if focused {
        let (col, row) = cursor_offset(&app.input);
        frame.set_cursor_position(Position {
            x: inner.x + col,
            y: inner.y + row,
        });
    }
}

fn input_text(app: &App) -> Vec<Line<'static>> {
    if app.input.is_empty() {
        return vec![Line::from(Span::styled(
            "type a message, Enter to send",
            Style::default().fg(Color::Gray),
        ))];
    }
    app.input
        .split('\n')
        .map(|line| Line::from(Span::styled(line.to_owned(), Style::default().fg(Color::White))))
        .collect()
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = match (app.focus, app.view) {
        (Focus::Input, _) => "Enter send · Alt+Enter newline · Esc back to list",
        (Focus::List, View::Events) => {
            "j/k move · space fold · m minor · r raw · l log · i message · s stop · q quit"
        }
        (Focus::List, View::Raw) => "j/k scroll · g/G top/bottom · r events · l log · i message · q quit",
        (Focus::List, View::Log) => "j/k scroll · g/G top/bottom · l events · r raw · i message · q quit",
    };
    let footer = Paragraph::new(Line::from(Span::styled(
        format!(" {hints}"),
        Style::default().fg(Color::Gray),
    )));
    frame.render_widget(footer, area);
}

/// Input box height: borders plus the number of message lines, within bounds.
fn input_area_height(app: &App) -> u16 {
    let lines = app.input.split('\n').count().max(1);
    (lines as u16 + 2).clamp(3, 8)
}

/// Column and row of the cursor at the end of the message buffer.
fn cursor_offset(input: &str) -> (u16, u16) {
    let row = input.matches('\n').count() as u16;
    let last = input.rsplit('\n').next().unwrap_or("");
    (last.chars().count() as u16, row)
}

fn tag_color(tag: &str) -> Color {
    match tag {
        "agent" => Color::Green,
        "user" => Color::Cyan,
        "command" => Color::Yellow,
        "tool" => Color::Magenta,
        "plan" | "files" => Color::Blue,
        "error" | "malformed" => Color::Red,
        _ => Color::DarkGray,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AgentEvent, TokenUsage};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn header_shows_profile_and_status() {
        let app = App::new("codex", "s1");
        let screen = rendered(&app);
        assert!(screen.contains("styra"));
        assert!(screen.contains("codex"));
        assert!(screen.contains("running"));
    }

    #[test]
    fn header_shows_a_dot_indicating_running_vs_waiting() {
        let mut app = App::new("codex", "s1");
        assert!(rendered(&app).contains('●'));
        assert_eq!(status_color(&app.status), Color::Green);

        app.push_event(AgentEvent::TurnCompleted { usage: TokenUsage::default() });
        assert!(rendered(&app).contains("waiting"));
        assert_eq!(status_color(&app.status), Color::Yellow);
    }

    #[test]
    fn a_collapsed_entry_shows_its_summary_and_a_fold_marker() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::AgentMessage { text: "hello world".into() });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(screen.contains('▸'));
        assert!(screen.contains("agent"));
    }

    #[test]
    fn an_expanded_command_shows_detail_lines() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.expand_all();
        let screen = rendered(&app);
        assert!(screen.contains('▾'));
        assert!(screen.contains("24 passed"));
    }

    #[test]
    fn expanding_does_not_repeat_the_summary_as_the_first_detail_line() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.expand_all();
        let screen = rendered(&app);
        // "$ cargo test" is the detail body's first line and only restates
        // the summary shown just above it; it must not be printed again.
        assert!(!screen.contains("$ cargo test"));
        assert!(screen.contains("24 passed"));
    }

    #[test]
    fn usage_is_shown_once_recorded() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage { input_tokens: 12, output_tokens: 3, ..Default::default() },
        });
        let screen = rendered(&app);
        assert!(screen.contains("in 12"));
    }

    #[test]
    fn minor_events_are_omitted_from_the_list_when_hidden() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::ThreadStarted { thread_id: "t-1".into() });
        app.push_event(AgentEvent::AgentMessage { text: "hello world".into() });
        app.toggle_minor();
        let screen = rendered(&app);
        assert!(!screen.contains("t-1"));
        assert!(screen.contains("hello world"));
    }

    #[test]
    fn footer_hints_depend_on_focus() {
        let mut app = App::new("codex", "s1");
        assert!(rendered(&app).contains("i message"));
        app.enter_input();
        assert!(rendered(&app).contains("Enter send"));
    }

    #[test]
    fn raw_view_shows_wire_lines_with_direction_markers() {
        use crate::session::{Direction, RawLine};
        let mut app = App::new("codex", "s1");
        app.push_raw(RawLine {
            direction: Direction::ToAgent,
            text: r#"{"op":"user_input"}"#.into(),
        });
        app.push_raw(RawLine {
            direction: Direction::FromAgent,
            text: r#"{"type":"turn.started"}"#.into(),
        });
        app.toggle_raw();
        let screen = rendered(&app);
        assert!(screen.contains("raw"));
        assert!(screen.contains('»'));
        assert!(screen.contains('«'));
        assert!(screen.contains("turn.started"));
    }

    #[test]
    fn long_summary_lines_wrap_instead_of_being_clipped() {
        let mut app = App::new("codex", "s1");
        // Long enough that a single 80-column row (minus borders) could not
        // hold it; a fixed-width row concatenation of the test buffer would
        // otherwise cut this down to a handful of repetitions.
        app.push_event(AgentEvent::AgentMessage { text: "word ".repeat(40) });
        let screen = rendered(&app);
        assert!(
            screen.matches("word").count() > 20,
            "expected wrapped continuation lines, only found: {screen:?}"
        );
    }

    #[test]
    fn log_view_shows_entries_with_levels() {
        use crate::session::LogEntry;
        let mut app = App::new("codex", "s1");
        app.push_log(LogEntry::info("launching codex"));
        app.push_log(LogEntry::error("could not run the agent: bwrap missing"));
        app.toggle_log();
        let screen = rendered(&app);
        assert!(screen.contains("log"));
        assert!(screen.contains("info"));
        assert!(screen.contains("error"));
        assert!(screen.contains("bwrap missing"));
    }
}
