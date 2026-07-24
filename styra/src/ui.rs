//! Terminal rendering of [`App`] with ratatui.
//!
//! Three stacked regions: the event list (each entry a summary line that grows
//! inline when expanded), the message box, and a one-line status/help footer.
//! Rendering is a pure function of `App`; all state lives in [`crate::app`].

use crate::agent::SandboxLayout;
use crate::app::{App, Entry, Focus, Status, View};
use crate::event::{DetailBlock, AgentEvent};
use crate::journal::SessionSummary;
use crate::session::{Direction as WireDirection, LogLevel};
use driva::{Mount, MountAccess};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::path::{Path, PathBuf};

/// Cap on detail lines shown for one expanded entry, so a single noisy command
/// cannot bury the rest of the session.
const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";
/// Backdrop painted behind a selected list row (including its expanded detail
/// lines, if any), via the list's `highlight_style`. Deliberately not white —
/// with `Modifier::REVERSED`, a `White` foreground would reverse onto the
/// background and flip the whole selected row to a glaring full-white fill.
const DETAIL_BG: Color = Color::DarkGray;

/// Color coding for the status dot, so running vs. idle for input reads at
/// a glance instead of requiring the operator to read the label text.
fn status_color(status: &Status) -> Color {
    match status {
        Status::Pending => Color::Blue,
        Status::Running => Color::Yellow,
        Status::Idle => Color::Green,
        Status::Stopped => Color::DarkGray,
        Status::Ended { error: Some(_), .. } => Color::Red,
        Status::Ended { .. } => Color::DarkGray,
    }
}

/// Build a block title of the form " styra · profile · ● status[ · suffix] ".
///
/// The plain-text spans are explicitly colored rather than left unstyled:
/// an unstyled span only patches over whatever the block's border already
/// painted underneath it, so when the border dims to `DarkGray` for an
/// unfocused panel, unstyled title text would dim right along with it and
/// become hard to read.
fn title_line(profile: &str, status: &Status, suffix: Option<&str>) -> Line<'static> {
    let color = status_color(status);
    let text_style = Style::default().fg(Color::Gray);
    let mut spans = vec![
        Span::styled(format!(" styra · {profile} · "), text_style),
        Span::styled("● ", Style::default().fg(color)),
        Span::styled(status.label(), Style::default().fg(color).add_modifier(Modifier::BOLD)),
    ];
    spans.push(match suffix {
        Some(suffix) => Span::styled(format!(" · {suffix} "), text_style),
        None => Span::styled(" ", text_style),
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
        View::Transcript => render_transcript_view(frame, app, chunks[0]),
        View::Driva => render_driva(frame, app, chunks[0]),
    }
    render_input(frame, app, chunks[1]);
    render_footer(frame, app, chunks[2]);
}

/// Render the session picker screen: every stored session, newest first,
/// with `selected` highlighted. Standalone from [`App`] — the picker runs
/// before any session is loaded, so it has no state of its own to render.
pub fn render_picker(frame: &mut Frame, sessions: &[SessionSummary], selected: usize) {
    let area = frame.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" styra · choose a session · Enter open · q cancel ");

    if sessions.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no sessions found",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let items: Vec<ListItem> = sessions.iter().map(session_item).collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
    let mut state = ListState::default();
    state.select(Some(selected.min(sessions.len() - 1)));
    frame.render_stateful_widget(list, area, &mut state);
}

fn session_item(session: &SessionSummary) -> ListItem<'static> {
    let profile = session.profile.clone().unwrap_or_else(|| "unknown".into());
    ListItem::new(Line::from(vec![
        Span::styled(
            format!("{profile:<14} "),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{:<10} ", session.age), Style::default().fg(Color::Gray)),
        Span::styled(session.id.clone(), Style::default().fg(Color::White)),
    ]))
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

/// A quick way to read the current session as a plain-text transcript,
/// rendered fresh from the decoded events each frame with genta's
/// `render_events` — the same rendering `journal::render_transcript` uses to
/// seed a switched-to session, just over the in-memory entries instead of a
/// stored journal. Unlike the raw/log views, it reads as a document from the
/// start rather than anchoring to the tail.
///
/// Follows `app.show_minor`, same as the event list; since this recomputes
/// from `app.entries` fresh every frame rather than caching anything,
/// toggling `m` while the transcript is open re-renders it with no extra
/// wiring needed.
fn render_transcript_view(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.profile_name, &app.status, Some("transcript")));

    if app.entries.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  nothing to render yet",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let events: Vec<AgentEvent> = app.entries.iter().map(|entry| entry.event.clone()).collect();
    let text = crate::render::render_events(&events, false, app.show_minor);
    let lines: Vec<Line<'static>> = text
        .lines()
        .map(|line| Line::from(Span::styled(line.to_owned(), Style::default().fg(Color::White))))
        .collect();

    let viewport = area.height.saturating_sub(2) as usize;
    let max_start = lines.len().saturating_sub(viewport) as u16;
    let start = app.transcript_scroll.min(max_start);
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
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

/// What the session was actually launched with: the isolation backend, the
/// command it runs, and the mount/network policy enforced around it — an
/// answer to "what can this agent touch" without having to go dig through
/// `main.rs`.
fn render_driva(frame: &mut Frame, app: &App, area: Rect) {
    let border_style = if app.focus == Focus::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title_line(&app.profile_name, &app.status, Some("driva")));

    let Some(options) = &app.driva_options else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no live session yet; nothing to describe",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    };

    let mut lines = vec![
        driva_field_line("backend", &options.isolation_backend),
        driva_field_line("command", &options.command.join(" ")),
        driva_field_line("workdir", &options.working_directory.display().to_string()),
        driva_field_line("network", if options.network { "on" } else { "off" }),
        Line::from(""),
        Line::from(Span::styled(
            "mounts",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )),
    ];
    lines.extend(options.mounts.iter().map(mount_line));

    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn driva_field_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {label:<8} "),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(value.to_owned(), Style::default().fg(Color::White)),
    ])
}

fn mount_line(mount: &Mount) -> Line<'static> {
    match mount {
        Mount::Bind { source, destination, access } => {
            let (label, color) = match access {
                MountAccess::ReadWrite => ("rw", Color::Yellow),
                MountAccess::ReadOnly => ("ro", Color::Gray),
            };
            Line::from(vec![
                Span::styled(
                    format!("  {label} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} → {}", source.display(), destination.display()),
                    Style::default().fg(Color::White),
                ),
            ])
        }
        Mount::Temporary { destination } => Line::from(vec![
            Span::styled("  tmp ", Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD)),
            Span::styled(destination.display().to_string(), Style::default().fg(Color::White)),
        ]),
    }
}

/// The togglable side panel: the full, uncapped expanded content of the
/// selected entry, regardless of whether it is folded in the list.
fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" preview ", Style::default().fg(Color::Gray)));

    let Some(entry) = app.selected_entry() else {
        let empty = Paragraph::new(Line::from(Span::styled(
            "  no entry selected",
            Style::default().fg(Color::Gray),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    };

    let detail = detail_lines(&entry.event, None);
    let mut lines = vec![summary_line(entry, !detail.is_empty())];
    lines.extend(detail);
    if let AgentEvent::FileChanged { paths, .. } = &entry.event {
        lines.extend(file_content_lines(paths, app.workspace_root.as_deref()));
    }
    let paragraph = Paragraph::new(lines).block(block).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

/// Read back the current content of files a `FileChanged` event touched, so
/// the preview shows what changed rather than just the bare path list.
fn file_content_lines(paths: &[String], workspace_root: Option<&Path>) -> Vec<Line<'static>> {
    let Some(root) = workspace_root else {
        return vec![Line::from(Span::styled(
            format!("{DETAIL_INDENT}(workspace path unknown; file content unavailable)"),
            Style::default().fg(Color::Gray),
        ))];
    };

    let mut lines = Vec::new();
    for path in paths {
        lines.push(Line::from(Span::styled(
            format!("{DETAIL_INDENT}── {path} ──"),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        match std::fs::read_to_string(resolve_workspace_path(root, path)) {
            Ok(content) => {
                for line in content.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("{DETAIL_INDENT}{line}"),
                        Style::default().fg(Color::White),
                    )));
                }
            }
            Err(error) => {
                lines.push(Line::from(Span::styled(
                    format!("{DETAIL_INDENT}could not read file: {error}"),
                    Style::default().fg(Color::Red),
                )));
            }
        }
    }
    lines
}

/// Map a path as the agent reported it onto the host filesystem. A relative
/// path joins directly onto the host workspace root (the sandbox's working
/// directory mirrors it 1:1 through a bind mount); an absolute path inside
/// the sandbox's mount destination is rewritten onto that same host root.
fn resolve_workspace_path(root: &Path, reported: &str) -> PathBuf {
    let reported_path = Path::new(reported);
    if reported_path.is_absolute() {
        return match reported_path.strip_prefix(&SandboxLayout::default().workspace) {
            Ok(relative) => root.join(relative),
            Err(_) => reported_path.to_path_buf(),
        };
    }
    root.join(reported_path)
}

fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let area = if app.show_preview {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_preview(frame, app, chunks[1]);
        chunks[0]
    } else {
        area
    };

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
    // An explicit background rather than `Modifier::REVERSED`: reversing
    // would swap a `White` foreground (summary and detail text alike) into
    // the background, flashing the selected row to a glaring full white.
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(DETAIL_BG).add_modifier(Modifier::BOLD));
    let mut state = ListState::default();
    let position = visible
        .iter()
        .position(|(idx, _)| *idx == app.selected)
        .or_else(|| visible.iter().rposition(|(idx, _)| *idx < app.selected));
    state.select(position);
    frame.render_stateful_widget(list, area, &mut state);
}

fn entry_item(entry: &Entry, width: usize) -> ListItem<'static> {
    let detail = detail_lines(&entry.event, Some(MAX_DETAIL_LINES));
    let mut lines = vec![summary_line(entry, !detail.is_empty())];
    if entry.expanded {
        lines.extend(detail);
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

/// `has_detail` is false when the entry has nothing beyond its summary (e.g.
/// a bare `turn started` marker); folding is meaningless there, so no arrow
/// is shown at all rather than one that never does anything when pressed.
fn summary_line(entry: &Entry, has_detail: bool) -> Line<'static> {
    let marker = match (has_detail, entry.expanded) {
        (false, _) => " ",
        (true, true) => "▾",
        (true, false) => "▸",
    };
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

/// The expandable body of an entry. `cap` bounds how many lines are shown
/// inline in the list (so one noisy command cannot bury the rest of the
/// session); pass `None` for the preview panel, which shows the body in full.
fn detail_lines(event: &AgentEvent, cap: Option<usize>) -> Vec<Line<'static>> {
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
    if let Some(cap) = cap {
        if lines.len() > cap {
            let hidden = lines.len() - cap;
            lines.truncate(cap);
            lines.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}… {hidden} more lines"),
                Style::default().fg(Color::Gray),
            )));
        }
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
        .title(Span::styled(title, Style::default().fg(Color::Gray)));
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
            "j/k move · space fold · C collapse all · m minor · p preview · t transcript · r raw · l log · d driva · i message · s stop · V switch · q quit"
        }
        (Focus::List, View::Raw) => {
            "j/k scroll · g/G top/bottom · r events · l log · t transcript · d driva · i message · s stop · V switch · q quit"
        }
        (Focus::List, View::Log) => {
            "j/k scroll · g/G top/bottom · l events · r raw · t transcript · d driva · i message · s stop · V switch · q quit"
        }
        (Focus::List, View::Transcript) => {
            "j/k scroll · g/G top/bottom · t events · r raw · l log · d driva · i message · s stop · V switch · q quit"
        }
        (Focus::List, View::Driva) => {
            "d events · r raw · l log · t transcript · i message · s stop · V switch · q quit"
        }
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
    use ratatui::buffer::Buffer;
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

    /// The (x, y) of `needle`'s first character in the buffer. Column-based
    /// rather than a byte offset into a joined `String`: title rows carry
    /// multi-byte box-drawing and separator glyphs (`┌`, `·`, `●`) ahead of
    /// plain-ASCII text, so a byte offset from `str::find` would overshoot the
    /// actual column whenever the needle sits after one of those.
    fn find_column(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let needle_chars: Vec<char> = needle.chars().collect();
        for y in 0..buffer.area.height {
            let symbols: Vec<&str> = (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect();
            let found = (0..symbols.len()).find(|&start| {
                needle_chars
                    .iter()
                    .enumerate()
                    .all(|(i, &ch)| symbols.get(start + i).and_then(|s| s.chars().next()) == Some(ch))
            });
            if let Some(x) = found {
                return (x as u16, y);
            }
        }
        panic!("no cell contains {needle:?}");
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
    fn header_text_stays_legible_when_the_panel_is_unfocused() {
        // An unstyled span only patches over whatever the block's border
        // already painted underneath it, so title text left unstyled would
        // inherit the border's `DarkGray` the moment the panel loses focus.
        let mut app = App::new("codex", "s1");
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Input);

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let (x, y) = find_column(&buffer, "styra");
        let cell = buffer.cell((x, y)).unwrap();
        assert_ne!(
            cell.style().fg,
            Some(Color::DarkGray),
            "title text must not inherit the dimmed unfocused border color"
        );
    }

    #[test]
    fn expanded_and_selected_content_uses_a_gray_backdrop_not_white() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::AgentMessage { text: "hello\nworld".into() });
        // `push_event` leaves the newest entry both selected (via follow) and,
        // once expanded, the case that used to flip to a reversed-white fill.
        app.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let backgrounds: Vec<Color> = buffer
            .content()
            .iter()
            .map(|cell| cell.style().bg.unwrap_or(Color::Reset))
            .collect();

        assert!(!backgrounds.contains(&Color::White));
        assert!(backgrounds.contains(&Color::DarkGray));
    }

    #[test]
    fn only_the_selected_entrys_expanded_content_gets_a_gray_backdrop() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::AgentMessage { text: "one\ntwo".into() });
        app.push_event(AgentEvent::AgentMessage { text: "three\nfour".into() });
        // `push_event` leaves the second (last) entry selected via follow;
        // both get expanded, but only the selected one should be highlighted.
        app.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let row_containing = |text: &str| -> u16 {
            (0..buffer.area.height)
                .find(|&y| {
                    let row: String = (0..buffer.area.width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect();
                    row.contains(text)
                })
                .unwrap_or_else(|| panic!("no row contains {text:?}"))
        };
        let row_has_gray_backdrop = |y: u16| {
            (0..buffer.area.width).any(|x| {
                buffer.cell((x, y)).unwrap().style().bg == Some(Color::DarkGray)
            })
        };

        let unselected_detail_row = row_containing("two");
        let selected_detail_row = row_containing("four");
        assert!(!row_has_gray_backdrop(unselected_detail_row));
        assert!(row_has_gray_backdrop(selected_detail_row));
    }

    #[test]
    fn header_shows_a_dot_indicating_running_vs_idle() {
        let mut app = App::new("codex", "s1");
        assert!(rendered(&app).contains('●'));
        assert_eq!(status_color(&app.status), Color::Yellow);

        app.push_event(AgentEvent::TurnCompleted { usage: TokenUsage::default() });
        assert!(rendered(&app).contains("idle"));
        assert_eq!(status_color(&app.status), Color::Green);
    }

    #[test]
    fn a_collapsed_entry_with_more_to_show_has_a_fold_marker() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::AgentMessage { text: "hello world\nmore detail".into() });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(screen.contains('▸'));
        assert!(screen.contains("agent"));
    }

    #[test]
    fn an_entry_with_nothing_beyond_its_summary_has_no_fold_marker() {
        let mut app = App::new("codex", "s1");
        // A single-line agent message: its detail body is identical to the
        // summary already shown, so there is nothing left to expand into.
        app.push_event(AgentEvent::AgentMessage { text: "hello world".into() });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(!screen.contains('▸'));
        assert!(!screen.contains('▾'));
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
        // Hidden by default; no toggle needed to get here.
        assert!(!app.show_minor);
        let screen = rendered(&app);
        assert!(!screen.contains("t-1"));
        assert!(screen.contains("hello world"));
    }

    #[test]
    fn footer_hints_depend_on_focus() {
        let mut app = App::new("codex", "s1");
        // The full hint line is longer than the 80-column test terminal, so
        // check a marker near its start rather than one that may be clipped.
        assert!(rendered(&app).contains("j/k move"));
        app.enter_input();
        assert!(rendered(&app).contains("Enter send"));
    }

    #[test]
    fn message_box_title_stays_legible_when_unfocused() {
        // Same bug as the header/preview titles: an unstyled title patches
        // onto the border paint underneath it, so it dimmed to `DarkGray`
        // whenever the message box lost focus.
        let app = App::new("codex", "s1");
        assert_eq!(app.focus, Focus::List);

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let (x, y) = find_column(&buffer, "message");
        let cell = buffer.cell((x, y)).unwrap();
        assert_ne!(cell.style().fg, Some(Color::DarkGray));
    }

    #[test]
    fn footer_advertises_the_collapse_all_shortcut() {
        let app = App::new("codex", "s1");
        assert!(rendered(&app).contains("collapse all"));
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
    fn driva_view_shows_the_launch_policy_or_a_placeholder_before_launch() {
        use crate::session::DrivaOptions;
        use driva::{Mount, MountAccess};

        let mut app = App::new("codex", "s1");
        app.toggle_driva();
        let placeholder = rendered(&app);
        assert!(placeholder.contains("no live session"));

        app.set_driva_options(DrivaOptions {
            isolation_backend: "bwrap".into(),
            command: vec!["codex".into(), "app-server".into()],
            working_directory: PathBuf::from("/tmp/styra/workspace"),
            network: false,
            mounts: vec![Mount::Bind {
                source: PathBuf::from("/home/op/project"),
                destination: PathBuf::from("/tmp/styra/workspace"),
                access: MountAccess::ReadWrite,
            }],
        });
        let screen = rendered(&app);
        assert!(screen.contains("driva"));
        assert!(screen.contains("bwrap"));
        assert!(screen.contains("codex app-server"));
        assert!(screen.contains("off"));
        assert!(screen.contains("/home/op/project"));
        assert!(screen.contains("/tmp/styra/workspace"));
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
    fn preview_panel_shows_full_content_of_the_selected_entry_when_toggled() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        // The preview must not depend on the entry being expanded in the list.
        assert!(!app.entries[0].expanded);
        assert!(!rendered(&app).contains("24 passed"));

        app.toggle_preview();
        let shown = rendered(&app);
        assert!(shown.contains("preview"));
        assert!(shown.contains("24 passed"));
    }

    #[test]
    fn preview_shows_the_current_content_of_a_changed_file() {
        let dir = std::env::temp_dir().join(format!(
            "styra-preview-file-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "line one\nline two").unwrap();

        let mut app = App::new("codex", "s1");
        app.set_workspace_root(dir.clone());
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            checkpoint: None,
            checkpoint_error: None,
        });
        app.toggle_preview();

        let screen = rendered(&app);
        assert!(screen.contains("notes.txt"));
        assert!(screen.contains("line one"));
        assert!(screen.contains("line two"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_text_is_never_highlighted() {
        let dir = std::env::temp_dir().join(format!(
            "styra-preview-nohighlight-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "line one\nline two").unwrap();

        let mut app = App::new("codex", "s1");
        app.set_workspace_root(dir.clone());
        // A FileChanged entry exercises both the ordinary detail body and the
        // file-content lines, the two sources of preview text.
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            checkpoint: None,
            checkpoint_error: None,
        });
        app.toggle_preview();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        // The preview occupies the right ~40% of the frame (the list's own
        // selection highlight lives to the left of that and is unaffected).
        // `Style::default()` renders as `Some(Color::Reset)`, not `None`, so
        // check for the specific highlight color rather than any `Some` bg.
        let preview_columns = 50..buffer.area.width;
        let has_highlight = preview_columns
            .flat_map(|x| (0..buffer.area.height).map(move |y| (x, y)))
            .any(|(x, y)| buffer.cell((x, y)).unwrap().style().bg == Some(Color::DarkGray));
        assert!(!has_highlight, "preview text should never carry a background highlight");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_title_stays_legible_against_its_always_dark_border() {
        // The preview panel's border is unconditionally `DarkGray` (it has no
        // separate focus state), so its unstyled title used to inherit that
        // same dim color from the border paint underneath it.
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::AgentMessage { text: "hello".into() });
        app.toggle_preview();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let (x, y) = find_column(&buffer, "preview");
        let cell = buffer.cell((x, y)).unwrap();
        assert_ne!(cell.style().fg, Some(Color::DarkGray));
    }

    #[test]
    fn preview_notes_an_unknown_workspace_instead_of_failing_to_read_a_file() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            checkpoint: None,
            checkpoint_error: None,
        });
        app.toggle_preview();
        assert!(rendered(&app).contains("workspace path unknown"));
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

    #[test]
    fn transcript_view_renders_the_current_session() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::UserMessage { text: "implement retry backoff".into() });
        app.push_event(AgentEvent::AgentMessage { text: "Added backoff, tests pass.".into() });
        app.toggle_transcript();
        let screen = rendered(&app);
        assert!(screen.contains("transcript"));
        assert!(screen.contains("implement retry backoff"));
        assert!(screen.contains("Added backoff"));
    }

    #[test]
    fn transcript_view_follows_the_minor_toggle_and_rerenders_when_flipped() {
        let mut app = App::new("codex", "s1");
        app.push_event(AgentEvent::ThreadStarted { thread_id: "t-1".into() });
        app.push_event(AgentEvent::AgentMessage { text: "hello world".into() });
        app.toggle_transcript();

        assert!(!app.show_minor);
        assert!(!rendered(&app).contains("t-1"));

        // Toggling minor visibility while the transcript is already open must
        // re-render it on the very next frame, not require reopening the view.
        app.toggle_minor();
        assert!(rendered(&app).contains("t-1"));
    }

    #[test]
    fn transcript_view_shows_a_placeholder_before_anything_happens() {
        let mut app = App::new("codex", "s1");
        app.toggle_transcript();
        assert!(rendered(&app).contains("nothing to render yet"));
    }

    fn picker_summary(id: &str, profile: Option<&str>, age: &str) -> SessionSummary {
        SessionSummary {
            id: id.into(),
            path: std::path::PathBuf::from(id),
            profile: profile.map(str::to_owned),
            age: age.into(),
            created_at_ms: None,
        }
    }

    fn rendered_picker(sessions: &[SessionSummary], selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render_picker(frame, sessions, selected)).unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn picker_lists_sessions_with_profile_and_age() {
        let sessions = vec![
            picker_summary("s-1", Some("codex"), "2m ago"),
            picker_summary("s-2", None, "3h ago"),
        ];
        let screen = rendered_picker(&sessions, 0);
        assert!(screen.contains("choose a session"));
        assert!(screen.contains("codex"));
        assert!(screen.contains("2m ago"));
        assert!(screen.contains("s-1"));
        assert!(screen.contains("unknown"));
        assert!(screen.contains("3h ago"));
        assert!(screen.contains("s-2"));
    }

    #[test]
    fn picker_shows_a_placeholder_when_there_are_no_sessions() {
        let screen = rendered_picker(&[], 0);
        assert!(screen.contains("no sessions found"));
    }

    #[test]
    fn picker_highlights_the_selected_session() {
        let sessions =
            vec![picker_summary("s-1", Some("codex"), "2m ago"), picker_summary("s-2", Some("codex"), "3h ago")];

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| render_picker(frame, &sessions, 1)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let row_containing = |text: &str| -> u16 {
            (0..buffer.area.height)
                .find(|&y| {
                    let row: String = (0..buffer.area.width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect();
                    row.contains(text)
                })
                .unwrap_or_else(|| panic!("no row contains {text:?}"))
        };
        let row_has_gray_backdrop = |y: u16| {
            (0..buffer.area.width)
                .any(|x| buffer.cell((x, y)).unwrap().style().bg == Some(Color::DarkGray))
        };

        assert!(!row_has_gray_backdrop(row_containing("s-1")));
        assert!(row_has_gray_backdrop(row_containing("s-2")));
    }
}
