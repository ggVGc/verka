//! The togglable preview panel and its full-screen variant: the full,
//! uncapped expanded content of the selected entry, regardless of whether it
//! is folded in the list.

use super::{message_text_color, summary_line, DETAIL_INDENT};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use std::path::{Path, PathBuf};
use styra_server::agent::SandboxLayout;
use styra_server::event::{AgentEvent, DetailBlock, PresentationMode};

pub(crate) fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let title = match app.preview_mode {
        PresentationMode::Pretty => " preview · pretty · v: raw ",
        PresentationMode::Raw => " preview · raw · v: pretty ",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(title, Style::default().fg(Color::Gray)));
    let lines = preview_lines(app);
    let scroll_limit = preview_scroll_limit(
        &lines,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    app.preview_scroll_limit.set(scroll_limit);
    let scroll = app.preview_scroll.min(scroll_limit);
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
    let scroll_limit = preview_scroll_limit(&lines, area.width, area.height);
    app.preview_scroll_limit.set(scroll_limit);
    let scroll = app.preview_scroll.min(scroll_limit);
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

pub(crate) fn preview_scroll_limit(lines: &[Line<'_>], width: u16, height: u16) -> u16 {
    // Use the same wrapping implementation as the widget below. Dividing a
    // line's display width by the panel width undercounts when word wrapping
    // moves a word to the next row and leaves unused cells on the previous
    // one, which made the final preview rows unreachable.
    let rendered_lines = Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1));
    rendered_lines
        .saturating_sub(usize::from(height))
        .min(usize::from(u16::MAX)) as u16
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

    let protocol = app.selection.provider.protocol();
    let mut lines = vec![summary_line(entry, entry.has_detail(), false, protocol)];
    for block in protocol.presented_detail(&entry.event, app.preview_mode) {
        lines.extend(presented_block_lines(
            block,
            message_text_color(entry.event.tag()),
        ));
    }
    if let AgentEvent::FileChanged { paths, .. } = &entry.event {
        lines.extend(file_content_lines(paths, app.workspace_root.as_deref()));
    }
    lines
}

fn presented_block_lines(block: DetailBlock, text_color: Color) -> Vec<Line<'static>> {
    let (text, language) = match block {
        DetailBlock::Text(text) => (text, None),
        DetailBlock::Code { language, text } => (text, language),
    };
    text.lines()
        .map(|line| {
            if language.as_deref() == Some("bash") {
                let mut spans = vec![Span::styled(
                    DETAIL_INDENT.to_owned(),
                    Style::default().fg(Color::White),
                )];
                spans.extend(bash_spans(line));
                return Line::from(spans);
            }
            let color = if line.starts_with('+') && !line.starts_with("+++") {
                Color::Green
            } else if line.starts_with('-') && !line.starts_with("---") {
                Color::Red
            } else if line.starts_with("@@") {
                Color::Cyan
            } else {
                text_color
            };
            Line::from(Span::styled(
                format!("{DETAIL_INDENT}{}", line.replace('\t', "    ")),
                Style::default().fg(color),
            ))
        })
        .collect()
}

/// Small shell highlighter for command previews. Genta identifies the code as
/// Bash; Styra owns the terminal palette.
fn bash_spans(line: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        if rest.starts_with('#') {
            spans.push(Span::styled(
                rest.to_owned(),
                Style::default().fg(Color::DarkGray),
            ));
            break;
        }
        let first = rest.chars().next().unwrap();
        let (len, color) = if first == '\'' || first == '"' {
            let end = rest[1..]
                .find(first)
                .map(|offset| offset + 2)
                .unwrap_or(rest.len());
            (end, Color::Green)
        } else if first.is_whitespace() {
            (
                rest.find(|ch: char| !ch.is_whitespace())
                    .unwrap_or(rest.len()),
                Color::White,
            )
        } else {
            let end = rest
                .find(|ch: char| ch.is_whitespace() || "|&;<>".contains(ch))
                .unwrap_or(rest.len());
            if end == 0 {
                (first.len_utf8(), Color::Magenta)
            } else {
                let token = &rest[..end];
                let color = if token.starts_with('-') {
                    Color::Cyan
                } else if token.contains('$') {
                    Color::Yellow
                } else {
                    Color::White
                };
                (end, color)
            }
        };
        spans.push(Span::styled(
            rest[..len].to_owned(),
            Style::default().fg(color),
        ));
        rest = &rest[len..];
    }
    spans
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
    fn preview_scroll_limit_accounts_for_word_wrapping() {
        // At six columns this wraps onto three rows ("aaa", "bbb", "ccc"),
        // even though its total display width divided by six only rounds to
        // two. The last row must remain reachable in a one-row viewport.
        let lines = vec![Line::from("aaa bbb ccc")];
        assert_eq!(preview_scroll_limit(&lines, 6, 1), 2);
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
    fn file_diff_preview_toggles_between_minimal_and_raw_output() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some(
                "diff --git a/src/lib.rs b/src/lib.rs\nindex 123..456 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n context\n-old\n+new"
                    .into(),
            ),
            checkpoint: None,
            checkpoint_error: None,
        });
        app.toggle_preview();

        let minimal = rendered(&app);
        assert!(minimal.contains("pretty"));
        assert!(minimal.contains("-old"));
        assert!(minimal.contains("+new"));
        assert!(!minimal.contains("diff --git"));
        assert!(!minimal.contains("@@ -1,3"));
        assert!(!minimal.contains("index 123"));
        assert!(!minimal.contains(" context"));

        app.toggle_preview_mode();
        let raw = rendered(&app);
        assert!(raw.contains("raw"));
        assert!(raw.contains("diff --git"));
        assert!(raw.contains("@@ -1,3"));
        assert!(raw.contains("index 123"));
        assert!(raw.contains(" context"));
    }

    #[test]
    fn compact_provider_diff_still_changes_visibly_between_modes() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ edit @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        });

        let minimal = preview_lines(&app);
        let minimal = minimal
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(!minimal.contains("@@ edit @@"));
        assert!(minimal.contains("-old"));
        assert!(minimal.contains("+new"));

        app.toggle_preview_mode();
        let raw = preview_lines(&app);
        let raw = raw
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(raw.contains("@@ edit @@"));
        assert!(raw.contains("-old"));
        assert!(raw.contains("+new"));
    }

    #[test]
    fn claude_bash_toggles_between_highlighted_command_and_raw_json() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("claude").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"cargo test --all","description":"run the suite"}"#.into(),
        });

        let pretty = preview_lines(&app);
        let pretty_text = pretty
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(pretty_text.contains("cargo test --all"));
        assert!(!pretty_text.contains("description"));
        assert!(pretty
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.fg == Some(Color::Cyan)));

        app.toggle_preview_mode();
        let raw = preview_lines(&app)
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(raw.contains("description"));
    }

    #[test]
    fn codex_bash_toggles_between_highlighted_command_and_wrapper() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::CommandStarted {
            command: "/usr/bin/bash -lc 'cargo test --all'".into(),
        });

        let pretty = preview_lines(&app);
        let pretty_text = pretty
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(pretty_text.contains("cargo test --all"));
        assert!(!pretty_text.contains("/usr/bin/bash"));
        assert!(pretty
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.fg == Some(Color::Cyan)));

        app.toggle_preview_mode();
        let raw = preview_lines(&app)
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(raw.contains("/usr/bin/bash -lc"));
    }

    #[test]
    fn diff_preview_never_emits_literal_terminal_tabs() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::DiffUpdated {
            diff: "@@\n-\told\n+\tnew".into(),
        });

        let lines = preview_lines(&app);
        assert!(!lines
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.content.contains('\t')));
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
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
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
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        // The preview occupies the right ~40% of the frame (the list's own
        // selection highlight lives to the left of that and is unaffected).
        // `Style::default()` renders as `Some(Color::Reset)`, not `None`, so
        // check for the specific highlight color rather than any `Some` bg.
        let preview_columns = 50..buffer.area.width;
        let has_highlight = preview_columns
            .flat_map(|x| (0..buffer.area.height).map(move |y| (x, y)))
            .any(|(x, y)| {
                buffer.cell((x, y)).unwrap().style().bg == Some(super::super::SELECTION_BG)
            });
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
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
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
