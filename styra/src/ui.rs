//! Terminal rendering of [`App`] with ratatui.
//!
//! Three stacked regions: the event list (each entry a summary line that grows
//! inline when expanded), the message box, and a one-line status/help footer.
//! Rendering is a pure function of `App`; all state lives in [`crate::app`].

use crate::app::{App, Entry, Focus};
use crate::event::{DetailBlock, StyraEvent};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

/// Cap on detail lines shown for one expanded entry, so a single noisy command
/// cannot bury the rest of the session.
const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";

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

    render_list(frame, app, chunks[0]);
    render_input(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
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
    let title = format!(
        " styra · {} · {} ",
        app.profile_name,
        app.status.label()
    );
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title)
        .title_bottom(Line::from(usage).right_aligned());

    if app.entries.is_empty() {
        let empty = Paragraph::new(Line::from(vec![Span::styled(
            "  waiting for the agent — press i to send a message",
            Style::default().fg(Color::DarkGray),
        )]))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = app.entries.iter().map(entry_item).collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    let mut state = ListState::default();
    state.select(Some(app.selected.min(app.entries.len().saturating_sub(1))));
    frame.render_stateful_widget(list, area, &mut state);
}

fn entry_item(entry: &Entry) -> ListItem<'static> {
    let mut lines = vec![summary_line(entry)];
    if entry.expanded {
        lines.extend(detail_lines(&entry.event));
    }
    ListItem::new(lines)
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
        Span::raw(entry.event.summary()),
    ])
}

fn detail_lines(event: &StyraEvent) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in event.detail() {
        match block {
            DetailBlock::Text(text) => {
                for line in text.lines() {
                    lines.push(Line::from(format!("{DETAIL_INDENT}{line}")));
                }
            }
            DetailBlock::Code { text, .. } => {
                for line in text.lines() {
                    lines.push(Line::from(vec![Span::styled(
                        format!("{DETAIL_INDENT}{line}"),
                        Style::default().fg(Color::Gray),
                    )]));
                }
            }
        }
    }
    if lines.len() > MAX_DETAIL_LINES {
        let hidden = lines.len() - MAX_DETAIL_LINES;
        lines.truncate(MAX_DETAIL_LINES);
        lines.push(Line::from(Span::styled(
            format!("{DETAIL_INDENT}… {hidden} more lines"),
            Style::default().fg(Color::DarkGray),
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
            Style::default().fg(Color::DarkGray),
        ))];
    }
    app.input
        .split('\n')
        .map(|line| Line::from(line.to_owned()))
        .collect()
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let hints = match app.focus {
        Focus::List => "j/k move · space fold · i message · s stop · q quit",
        Focus::Input => "Enter send · Alt+Enter newline · Esc back to list",
    };
    let footer = Paragraph::new(Line::from(Span::styled(
        format!(" {hints}"),
        Style::default().fg(Color::DarkGray),
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
    use crate::event::{StyraEvent, TokenUsage};
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
    fn a_collapsed_entry_shows_its_summary_and_a_fold_marker() {
        let mut app = App::new("codex", "s1");
        app.push_event(StyraEvent::AgentMessage { text: "hello world".into() });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(screen.contains('▸'));
        assert!(screen.contains("agent"));
    }

    #[test]
    fn an_expanded_command_shows_detail_lines() {
        let mut app = App::new("codex", "s1");
        app.push_event(StyraEvent::CommandCompleted {
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
    fn usage_is_shown_once_recorded() {
        let mut app = App::new("codex", "s1");
        app.push_event(StyraEvent::TurnCompleted {
            usage: TokenUsage { input_tokens: 12, output_tokens: 3, ..Default::default() },
        });
        let screen = rendered(&app);
        assert!(screen.contains("in 12"));
    }

    #[test]
    fn footer_hints_depend_on_focus() {
        let mut app = App::new("codex", "s1");
        assert!(rendered(&app).contains("i message"));
        app.enter_input();
        assert!(rendered(&app).contains("Enter send"));
    }
}
