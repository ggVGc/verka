//! The main event list: each entry a summary line that grows inline when
//! expanded, plus the empty-list start screen and the trailing status tail.

use super::markdown::markdown_line_spans;
use super::{
    message_text_color, render_preview, tag_color, title_line, DETAIL_INDENT, MAX_DETAIL_LINES,
    SELECTION_BG,
};
use crate::app::{App, Entry, Focus, Status};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use styra_server::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};

pub(crate) fn render_list(frame: &mut Frame, app: &App, area: Rect) {
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
        .title(title_line(&app.launch_label(), &app.status, None))
        .title_bottom(Line::from(usage).right_aligned());

    if app.entries.is_empty() {
        // Before anything is launched, the empty list is the start screen: the
        // one moment the agent, model, and effort are still open, so it says
        // what they are and how to change them instead of only waiting.
        let lines = if app.can_configure_launch() {
            vec![
                Line::from(vec![
                    Span::styled("  launching with ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        app.selection.name(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    "  press L to choose the default agent, model, and effort — or i to write the first message",
                    Style::default().fg(Color::Gray),
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "  waiting for the agent — press i to send a message",
                Style::default().fg(Color::Gray),
            ))]
        };
        frame.render_widget(Paragraph::new(lines).block(block), area);
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
    let mut items: Vec<ListItem> = visible
        .iter()
        .map(|(_, entry)| entry_item(entry, width, app.selection.provider.protocol()))
        .collect();
    let item_heights: Vec<usize> = items.iter().map(ListItem::height).collect();
    items.push(ListItem::new(status_tail(app)));
    // An explicit background rather than `Modifier::REVERSED`: reversing
    // would swap a `White` foreground (summary and detail text alike) into
    // the background, flashing the selected row to a glaring full white.
    // No `Modifier::BOLD` either: `highlight_style` applies to the whole
    // selected row as one unit, so an expanded entry's detail body would be
    // forced bold right along with its summary line, with no way to exempt
    // it — the background alone is enough to mark the selection.
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(SELECTION_BG));
    let mut state = ListState::default();
    let position = visible
        .iter()
        .position(|(idx, _)| *idx == app.selected)
        .or_else(|| visible.iter().rposition(|(idx, _)| *idx < app.selected));
    let viewport_height = area.height.saturating_sub(2) as usize;
    let offset = list_offset_with_scrolloff(
        app.list_offset.get(),
        position,
        &item_heights,
        viewport_height,
    );
    *state.offset_mut() = offset;
    state.select(position);
    frame.render_stateful_widget(list, area, &mut state);
    app.list_offset.set(state.offset());
}

/// Keep the selected item within a small margin of the viewport edges, like
/// vim's `scrolloff`. Heights are rendered rows rather than item counts so
/// wrapped summaries and expanded details do not break the margin.
fn list_offset_with_scrolloff(
    current: usize,
    selected: Option<usize>,
    heights: &[usize],
    viewport_height: usize,
) -> usize {
    let Some(selected) = selected else {
        return current.min(heights.len().saturating_sub(1));
    };
    if viewport_height == 0 {
        return selected;
    }

    let margin = 2.min(viewport_height.saturating_sub(1) / 2);
    let mut offset = current.min(selected);

    while offset < selected && heights[offset..selected].iter().sum::<usize>() < margin {
        offset = offset.saturating_sub(1);
        if offset == 0 {
            break;
        }
    }

    while offset < selected {
        let rows_through_selection = heights[offset..=selected].iter().sum::<usize>();
        if rows_through_selection <= viewport_height.saturating_sub(margin) {
            break;
        }
        offset += 1;
    }

    // Moving upward may have left the selection close to the top edge. Pull
    // earlier items back into view until the margin is restored.
    while offset > 0 {
        let rows_before = heights[offset..selected].iter().sum::<usize>();
        if rows_before >= margin {
            break;
        }
        offset -= 1;
    }
    offset
}

fn status_tail(app: &App) -> Line<'static> {
    let (text, color) = match app.status {
        Status::Pending => ("  … waiting for your first message", Color::DarkGray),
        Status::Idle => ("  ── idle · waiting for your message ──", Color::Green),
        Status::Stopped => (
            "  ── paused · waiting for your next message ──",
            Color::DarkGray,
        ),
        _ => return Line::default(),
    };
    Line::from(Span::styled(text, Style::default().fg(color)))
}

fn entry_item(entry: &Entry, width: usize, protocol: Protocol) -> ListItem<'static> {
    let mut lines = vec![summary_line(
        entry,
        entry.has_detail(),
        !entry.expanded,
        protocol,
    )];
    if entry.expanded {
        lines.extend(detail_lines(&entry.event, Some(MAX_DETAIL_LINES)));
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
/// `show_summary` is false while expanded (inline or in the preview panel):
/// the detail body that follows already carries the full, untruncated
/// content, so repeating the (possibly truncated) summary text above it
/// would just show the same thing twice.
pub(crate) fn summary_line(
    entry: &Entry,
    has_detail: bool,
    show_summary: bool,
    protocol: Protocol,
) -> Line<'static> {
    let marker = match (has_detail, entry.expanded) {
        (false, _) => " ",
        (true, true) => "▾",
        (true, false) => "▸",
    };
    let tag = entry.event.tag();
    let is_conversation = matches!(
        entry.event,
        AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
    );
    let row_lead = if is_conversation { "" } else { "  " };
    let display_tag = match &entry.event {
        AgentEvent::UserMessage { .. } => "»",
        AgentEvent::AgentMessage { .. } => "«",
        AgentEvent::ToolStarted { name, .. } | AgentEvent::ToolCompleted { name, .. } => {
            name.as_str()
        }
        _ => tag,
    };
    // A completed tool call gets a checkmark/cross prefix so it does not read
    // like the running row it replaced. Keep a successful checkmark green
    // without painting the command text itself as a status color.
    let (summary_style, prefix, prefix_style) = match &entry.event {
        AgentEvent::ToolCompleted { status, .. } if status == "error" => {
            (
                Style::default().fg(Color::Red),
                "✗ ",
                Style::default().fg(Color::Red),
            )
        }
        AgentEvent::ToolCompleted { .. } => (
            Style::default().fg(Color::White),
            "✓ ",
            Style::default().fg(Color::Green),
        ),
        _ if tag == "command" => (
            Style::default().fg(tag_color(tag)),
            "",
            Style::default(),
        ),
        _ => (
            Style::default().fg(message_text_color(tag)),
            "",
            Style::default(),
        ),
    };
    let mut spans = vec![
        Span::raw(row_lead),
        Span::styled(
            if is_conversation {
                format!("{display_tag} ")
            } else {
                format!("{display_tag:<8}")
            },
            Style::default()
                .fg(tag_color(tag))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if show_summary {
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix, prefix_style));
        }
        let summary = file_action_summary(&entry.event).unwrap_or_else(|| {
            entry
                .event
                .presented_summary(protocol, PresentationMode::Pretty)
        });
        let summary = match &entry.event {
            AgentEvent::ToolStarted { name, .. } | AgentEvent::ToolCompleted { name, .. } => {
                summary
                    .strip_prefix(name)
                    .unwrap_or(&summary)
                    .trim_start_matches(": ")
            }
            _ => &summary,
        };
        spans.extend(super::markdown::parse_inline_spans(summary, summary_style));
    }
    if has_detail {
        spans.push(Span::styled(
            format!(" {marker}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

/// File-event summaries should say what happened, not merely repeat paths
/// under an opaque `files` tag. Providers do not always report a change kind,
/// so unified-diff creation/deletion markers are used when present and the
/// honest fallback is "changed".
fn file_action_summary(event: &AgentEvent) -> Option<String> {
    let (paths, diff) = match event {
        AgentEvent::FileChanged { paths, diff, .. } => (paths.clone(), diff.as_deref()),
        AgentEvent::DiffUpdated { diff } => {
            let paths = event
                .summary()
                .split(", ")
                .map(str::to_owned)
                .collect::<Vec<_>>();
            (paths, Some(diff.as_str()))
        }
        _ => return None,
    };
    let action = match diff {
        Some(diff) => {
            let added =
                diff.contains("new file mode") || diff.lines().any(|line| line == "--- /dev/null");
            let deleted = diff.contains("deleted file mode")
                || diff.lines().any(|line| line == "+++ /dev/null");
            match (added, deleted) {
                (true, false) => "added",
                (false, true) => "deleted",
                (false, false) => "modified",
                (true, true) => "changed",
            }
        }
        None => "changed",
    };
    Some(format!("{action} {}", paths.join(", ")))
}

/// The expandable body of an entry. `cap` bounds how many lines are shown
/// inline in the list (so one noisy command cannot bury the rest of the
/// session); pass `None` for the preview panel, which shows the body in full.
pub(crate) fn detail_lines(event: &AgentEvent, cap: Option<usize>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text_color = message_text_color(event.tag());
    for block in event.detail() {
        match block {
            DetailBlock::Text(text) => {
                let base_style = Style::default().fg(text_color);
                for line in text.lines() {
                    let mut spans = vec![Span::styled(DETAIL_INDENT.to_owned(), base_style)];
                    spans.extend(markdown_line_spans(line, base_style));
                    lines.push(Line::from(spans));
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn list_scrolloff_moves_before_selection_reaches_an_edge() {
        let heights = vec![1; 20];

        assert_eq!(list_offset_with_scrolloff(0, Some(5), &heights, 8), 0);
        assert_eq!(list_offset_with_scrolloff(0, Some(6), &heights, 8), 1);
        assert_eq!(list_offset_with_scrolloff(5, Some(5), &heights, 8), 3);
    }

    #[test]
    fn list_scrolloff_counts_expanded_rows() {
        let heights = vec![1, 1, 5, 1, 1];

        assert_eq!(list_offset_with_scrolloff(0, Some(3), &heights, 8), 2);
    }
    use styra_server::event::TokenUsage;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app))
            .unwrap();
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
    fn expanded_and_selected_content_uses_a_gray_backdrop_not_white() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello\nworld".into(),
        });
        // `push_event` leaves the newest entry both selected (via follow) and,
        // once expanded, the case that used to flip to a reversed-white fill.
        app.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let backgrounds: Vec<Color> = buffer
            .content()
            .iter()
            .map(|cell| cell.style().bg.unwrap_or(Color::Reset))
            .collect();

        assert!(!backgrounds.contains(&Color::White));
        assert!(backgrounds.contains(&SELECTION_BG));
    }

    #[test]
    fn an_expanded_selected_entrys_detail_body_is_never_bold() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello\nworld".into(),
        });
        app.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let detail_row = (0..buffer.area.height)
            .find(|&y| {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect();
                row.contains("world")
            })
            .expect("no row contains the detail line");
        let is_bold = (0..buffer.area.width).any(|x| {
            buffer
                .cell((x, detail_row))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        });
        assert!(
            !is_bold,
            "an expanded, selected entry's detail body must not be forced bold"
        );
    }

    #[test]
    fn only_the_selected_entrys_expanded_content_gets_a_gray_backdrop() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "one\ntwo".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "three\nfour".into(),
        });
        // `push_event` leaves the second (last) entry selected via follow;
        // both get expanded, but only the selected one should be highlighted.
        app.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
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
                .any(|x| buffer.cell((x, y)).unwrap().style().bg == Some(SELECTION_BG))
        };

        let unselected_detail_row = row_containing("two");
        let selected_detail_row = row_containing("four");
        assert!(!row_has_gray_backdrop(unselected_detail_row));
        assert!(row_has_gray_backdrop(selected_detail_row));
    }

    #[test]
    fn a_collapsed_entry_with_more_to_show_has_a_fold_marker() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world\nmore detail".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(screen.contains('▸'));
        assert!(screen.contains('«'));
    }

    #[test]
    fn an_entry_with_nothing_beyond_its_summary_has_no_fold_marker() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        // A single-line agent message: its detail body is identical to the
        // summary already shown, so there is nothing left to expand into.
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(!screen.contains('▸'));
        assert!(!screen.contains('▾'));
    }

    #[test]
    fn file_entries_name_the_action_taken() {
        let changed = AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ -1 +1 @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        };
        assert_eq!(
            file_action_summary(&changed).as_deref(),
            Some("modified src/lib.rs")
        );

        let added = AgentEvent::DiffUpdated {
            diff: "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs"
                .into(),
        };
        assert_eq!(file_action_summary(&added).as_deref(), Some("added new.rs"));

        let deleted = AgentEvent::FileChanged {
            id: "f2".into(),
            paths: vec!["old.rs".into()],
            diff: Some("--- a/old.rs\n+++ /dev/null".into()),
            checkpoint: None,
            checkpoint_error: None,
        };
        assert_eq!(
            file_action_summary(&deleted).as_deref(),
            Some("deleted old.rs")
        );
    }

    #[test]
    fn a_truncated_single_line_summary_has_a_fold_marker_and_expands_to_the_full_text() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "z".repeat(500),
        });
        let collapsed = rendered(&app);
        assert!(collapsed.contains('…'));
        assert!(collapsed.contains('▸'));
        let collapsed_zs = collapsed.chars().filter(|&c| c == 'z').count();

        app.toggle_expand();
        let expanded = rendered(&app);
        assert!(expanded.contains('▾'));
        let expanded_zs = expanded.chars().filter(|&c| c == 'z').count();
        assert!(expanded_zs > collapsed_zs);
        // The full message appears exactly once — not the truncated summary
        // fragment followed by the whole message again.
        assert_eq!(expanded_zs, 500);
    }

    #[test]
    fn a_completed_tool_shows_a_checkmark_instead_of_just_the_word_completed() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "ok".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains('✓'));
        assert!(screen.contains("Bash"));
        assert!(screen.contains("cargo test"));
        assert!(!screen.contains("tool     "));
        assert_eq!(screen.matches("Bash").count(), 1);

        let line = summary_line(
            &app.entries[0],
            app.entries[0].has_detail(),
            true,
            app.selection.provider.protocol(),
        );
        let check = line
            .spans
            .iter()
            .find(|span| span.content.contains('✓'))
            .unwrap();
        let command = line
            .spans
            .iter()
            .find(|span| span.content.contains("cargo test"))
            .unwrap();
        assert_eq!(check.style.fg, Some(Color::Green));
        assert_eq!(command.style.fg, Some(Color::White));
    }

    #[test]
    fn a_failed_tool_shows_a_cross_instead_of_a_checkmark() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "error".into(),
            output: "boom".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains('✗'));
        assert!(!screen.contains('✓'));
    }

    #[test]
    fn an_expanded_command_shows_detail_lines() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
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
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.expand_all();
        let screen = rendered(&app);
        // Expanding shows the full detail body ("$ cargo test", the status
        // line, and the output) but the header above it drops the summary
        // text once expanded, so nothing here is printed twice.
        assert_eq!(screen.matches("cargo test").count(), 1);
        assert!(screen.contains("$ cargo test"));
        assert!(screen.contains("24 passed"));
    }

    #[test]
    fn usage_is_shown_once_recorded() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage {
                input_tokens: 12,
                output_tokens: 3,
                ..Default::default()
            },
        });
        let screen = rendered(&app);
        assert!(screen.contains("in 12"));
    }

    #[test]
    fn minor_events_are_omitted_from_the_list_when_hidden() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-1".into(),
            model: None,
            effort: None,
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world".into(),
        });
        // Hidden by default; no toggle needed to get here.
        assert!(!app.show_minor);
        let screen = rendered(&app);
        assert!(!screen.contains("t-1"));
        assert!(screen.contains("hello world"));
    }

    #[test]
    fn long_summary_lines_wrap_instead_of_being_clipped() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "word ".repeat(40),
        });
        let screen = rendered(&app);
        assert!(
            screen.matches("word").count() > 20,
            "expected wrapped continuation lines, only found: {screen:?}"
        );
    }

    /// Before anything is launched, the empty list must name the launch and
    /// say what would be launched and how to change it, since that is the only
    /// moment the choice is still open.
    #[test]
    fn the_start_screen_names_the_launch_and_how_to_change_it() {
        let selection = styra_server::agent::Selection::parse("claude:opus/max").unwrap();
        let app = App::pending(selection);
        let screen = rendered(&app);
        assert!(screen.contains("claude:opus/max"), "{screen}");
        assert!(screen.contains("press L to choose"), "{screen}");
        assert!(
            screen.contains("Ctrl+L"),
            "the start screen opens in input focus: {screen}"
        );

        // A launched session shows the plain waiting message instead: its agent
        // and model are settled, so there is nothing to offer choosing.
        let app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        let screen = rendered(&app);
        assert!(screen.contains("waiting for the agent"), "{screen}");
        assert!(!screen.contains("press L to choose"), "{screen}");
    }
}
