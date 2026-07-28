//! The togglable preview panel and its full-screen variant: the full,
//! uncapped expanded content of the selected entry, regardless of whether it
//! is folded in the list.

use super::{detail_lines, summary_line, DETAIL_INDENT};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::path::{Path, PathBuf};
use styra_server::agent::SandboxLayout;
use styra_server::event::AgentEvent;

pub(crate) fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(" preview ", Style::default().fg(Color::Gray)));
    let lines = preview_lines(app);
    let scroll = preview_scroll(
        &lines,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
        app.preview_scroll,
    );
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

/// The `P` shortcut's full-screen view of the selected entry: the same
/// uncapped content as the side panel, but with no border, title, or other
/// chrome at all — just the text, filling the whole terminal, so it can be
/// selected and copied cleanly.
pub(crate) fn render_fullscreen_preview(frame: &mut Frame, app: &App, area: Rect) {
    let lines = preview_lines(app);
    let scroll = preview_scroll(&lines, area.width, area.height, app.preview_scroll);
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

pub(crate) fn preview_scroll(lines: &[Line<'_>], width: u16, height: u16, requested: u16) -> u16 {
    let width = usize::from(width.max(1));
    let rendered_lines: usize = lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum();
    let max_scroll = rendered_lines.saturating_sub(usize::from(height)) as u16;
    requested.min(max_scroll)
}

/// Shared body for the side-panel and full-screen preview: the selected
/// entry's uncapped summary, detail, and (for a `FileChanged` entry) current
/// file content.
pub(crate) fn preview_lines(app: &App) -> Vec<Line<'static>> {
    let Some(entry) = app.selected_entry() else {
        return vec![Line::from(Span::styled(
            "  no entry selected",
            Style::default().fg(Color::Gray),
        ))];
    };

    let mut lines = vec![summary_line(entry, entry.has_detail(), false)];
    lines.extend(detail_lines(&entry.event, None));
    if let AgentEvent::FileChanged { paths, .. } = &entry.event {
        lines.extend(file_content_lines(paths, app.workspace_root.as_deref()));
    }
    lines
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
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| super::super::render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn find_column(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let needle_chars: Vec<char> = needle.chars().collect();
        for y in 0..buffer.area.height {
            let symbols: Vec<&str> = (0..buffer.area.width)
                .map(|x| buffer.cell((x, y)).unwrap().symbol())
                .collect();
            let found = (0..symbols.len()).find(|&start| {
                needle_chars.iter().enumerate().all(|(i, &ch)| {
                    symbols.get(start + i).and_then(|s| s.chars().next()) == Some(ch)
                })
            });
            if let Some(x) = found {
                return (x as u16, y);
            }
        }
        panic!("no cell contains {needle:?}");
    }

    #[test]
    fn preview_of_a_truncated_message_shows_the_full_text_only_once() {
        // The list pane keeps showing its own truncated summary line
        // regardless of the preview panel, so this checks the preview's own
        // content directly rather than the whole screen.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "z".repeat(500),
        });
        let zs: usize = preview_lines(&app)
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.chars().filter(|&c| c == 'z').count())
            .sum();
        assert_eq!(zs, 500);
    }

    #[test]
    fn preview_panel_shows_full_content_of_the_selected_entry_when_toggled() {
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
        // The preview must not depend on the entry being expanded in the list.
        assert!(!app.entries[0].expanded);
        assert!(!rendered(&app).contains("24 passed"));

        app.toggle_preview();
        let shown = rendered(&app);
        assert!(shown.contains("preview"));
        assert!(shown.contains("24 passed"));
    }

    #[test]
    fn fullscreen_preview_replaces_the_whole_main_region() {
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
        assert!(!rendered(&app).contains("24 passed"));

        app.toggle_fullscreen_preview();
        let shown = rendered(&app);
        // No chrome at all: no title bar, no message box, no footer hints —
        // just the entry's text, so it can be selected and copied cleanly.
        assert!(shown.contains("24 passed"));
        assert!(!shown.contains("styra"));
        assert!(!shown.contains("message"));
        assert!(!shown.contains("quit"));

        app.toggle_fullscreen_preview();
        let restored = rendered(&app);
        assert!(restored.contains("styra"));
        assert!(restored.contains("command"));
    }

    #[test]
    fn fullscreen_preview_has_no_border_or_title() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello".into(),
        });
        app.toggle_fullscreen_preview();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| super::super::render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let screen: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        for border_char in ['┌', '┐', '└', '┘', '─', '│'] {
            assert!(
                !screen.contains(border_char),
                "found border character {border_char:?}"
            );
        }
    }

    #[test]
    fn preview_shows_the_current_content_of_a_changed_file() {
        let dir =
            std::env::temp_dir().join(format!("styra-preview-file-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "line one\nline two").unwrap();

        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.set_workspace_root(dir.clone());
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            diff: None,
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

        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.set_workspace_root(dir.clone());
        // A FileChanged entry exercises both the ordinary detail body and the
        // file-content lines, the two sources of preview text.
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.toggle_preview();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| super::super::render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        // The preview occupies the right ~40% of the frame (the list's own
        // selection highlight lives to the left of that and is unaffected).
        // `Style::default()` renders as `Some(Color::Reset)`, not `None`, so
        // check for the specific highlight color rather than any `Some` bg.
        let preview_columns = 50..buffer.area.width;
        let has_highlight = preview_columns
            .flat_map(|x| (0..buffer.area.height).map(move |y| (x, y)))
            .any(|(x, y)| buffer.cell((x, y)).unwrap().style().bg == Some(super::super::SELECTION_BG));
        assert!(
            !has_highlight,
            "preview text should never carry a background highlight"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn preview_title_stays_legible_against_its_always_dark_border() {
        // The preview panel's border is unconditionally `DarkGray` (it has no
        // separate focus state), so its unstyled title used to inherit that
        // same dim color from the border paint underneath it.
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello".into(),
        });
        app.toggle_preview();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal.draw(|frame| super::super::render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let (x, y) = find_column(&buffer, "preview");
        let cell = buffer.cell((x, y)).unwrap();
        assert_ne!(cell.style().fg, Some(Color::DarkGray));
    }

    #[test]
    fn preview_notes_an_unknown_workspace_instead_of_failing_to_read_a_file() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.toggle_preview();
        assert!(rendered(&app).contains("workspace path unknown"));
    }
}
