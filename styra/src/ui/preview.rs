//! The togglable preview panel and its full-screen variant: the full,
//! uncapped expanded content of the selected entry, regardless of whether it
//! is folded in the list.

use super::{message_text_color, palette, summary_line, wrap_line, DETAIL_INDENT};
use crate::app::App;
use crate::preview::PreviewTarget;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;
use styra_server::event::{AgentEvent, DetailBlock, PresentationMode};

pub(crate) fn render_preview(frame: &mut Frame, app: &App, area: Rect) {
    let title = match (app.preview.mode(), app.preview.target()) {
        (PresentationMode::Pretty, PreviewTarget::Selection) => {
            " preview · pretty · v: raw · C: command "
        }
        (PresentationMode::Raw, PreviewTarget::Selection) => {
            " preview · raw · v: pretty · C: command "
        }
        (PresentationMode::Pretty, PreviewTarget::Command) => {
            " command · pretty · v: raw · C: selection "
        }
        (PresentationMode::Raw, PreviewTarget::Command) => {
            " command · raw · v: pretty · C: selection "
        }
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(Span::styled(
            title,
            Style::default().fg(palette::MUTED_TEXT),
        ));
    let lines = wrap_preview_lines(
        preview_lines(app),
        usize::from(area.width.saturating_sub(2)),
        preview_summary_indent(app),
    );
    let scroll_limit = preview_scroll_limit(
        &lines,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    app.preview.scroll.note_limit(scroll_limit);
    let scroll = app.preview.scroll.clamped();
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
    let lines = wrap_preview_lines(
        preview_lines(app),
        usize::from(area.width),
        preview_summary_indent(app),
    );
    let scroll_limit = preview_scroll_limit(&lines, area.width, area.height);
    app.preview.scroll.note_limit(scroll_limit);
    let scroll = app.preview.scroll.clamped();
    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

/// Word-wraps the preview's logical lines with the same hanging indent as
/// the expanded list entries (`entry_item` in `list.rs`), so a wrapped line
/// stays aligned with the text that follows its `«`/`»` marker or detail
/// indent instead of jumping to the left edge.
fn wrap_preview_lines(
    lines: Vec<Line<'static>>,
    width: usize,
    summary_indent: usize,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .flat_map(|(index, line)| {
            let continuation_indent = if index == 0 {
                summary_indent
            } else {
                DETAIL_INDENT.len()
            };
            wrap_line(line, width, continuation_indent)
        })
        .collect()
}

/// The summary row's hanging indent for a wrapped continuation: aligned
/// under the `«`/`»` marker for conversation entries, flush left otherwise —
/// matching `entry_item`'s `summary_indent` in `list.rs`.
fn preview_summary_indent(app: &App) -> usize {
    let is_conversation = matches!(
        app.preview_entry().map(|entry| &entry.event),
        Some(AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. })
    );
    if is_conversation {
        2
    } else {
        0
    }
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
/// entry's uncapped summary and detail.
pub(crate) fn preview_lines(app: &App) -> Vec<Line<'static>> {
    let Some(entry) = app.preview_entry() else {
        return vec![Line::from(Span::styled(
            "  no entry selected",
            Style::default().fg(palette::MUTED_TEXT),
        ))];
    };

    let protocol = app.selection.provider.protocol();
    let mut lines = vec![summary_line(
        entry,
        entry.expanded,
        entry.has_detail(),
        false,
        protocol,
    )];
    let mut blocks = protocol
        .presented_detail(&entry.event, app.preview.mode())
        .into_iter();
    if let Some(first) = blocks.next() {
        lines.extend(presented_block_lines(
            first,
            message_text_color(entry.event.tag()),
            app.preview.mode(),
        ));
        for block in blocks {
            lines.push(Line::from(""));
            lines.extend(presented_block_lines(
                block,
                message_text_color(entry.event.tag()),
                app.preview.mode(),
            ));
        }
    }
    lines
}

fn presented_block_lines(
    block: DetailBlock,
    text_color: Color,
    mode: PresentationMode,
) -> Vec<Line<'static>> {
    // Prose blocks carry the agent's markdown, so the pretty preview styles it
    // the same way the expanded list entry does. Code blocks (commands,
    // output, diffs) stay verbatim, and raw mode stays raw by definition.
    if mode == PresentationMode::Pretty {
        if let DetailBlock::Text(text) = &block {
            let base_style = Style::default().fg(text_color);
            return super::markdown::markdown_block_lines(text, base_style, DETAIL_INDENT);
        }
    }
    let (text, language) = match block {
        DetailBlock::Text(text) => (text, None),
        DetailBlock::Code { language, text } => (text, language),
    };
    text.lines()
        .map(|line| {
            if language.as_deref() == Some("bash") {
                let mut spans = vec![Span::styled(
                    DETAIL_INDENT.to_owned(),
                    Style::default().fg(palette::TEXT),
                )];
                spans.extend(bash_spans(line));
                return Line::from(spans);
            }
            let color = if line.starts_with('+') && !line.starts_with("+++") {
                palette::SUCCESS
            } else if line.starts_with('-') && !line.starts_with("---") {
                palette::ERROR
            } else if line.starts_with("@@") {
                palette::ACCENT
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
                Style::default().fg(palette::ADDITIONAL_INFO),
            ));
            break;
        }
        let first = rest.chars().next().unwrap();
        let (len, color) = if first == '\'' || first == '"' {
            let end = rest[1..]
                .find(first)
                .map(|offset| offset + 2)
                .unwrap_or(rest.len());
            (end, palette::SUCCESS)
        } else if first.is_whitespace() {
            (
                rest.find(|ch: char| !ch.is_whitespace())
                    .unwrap_or(rest.len()),
                palette::TEXT,
            )
        } else {
            let end = rest
                .find(|ch: char| ch.is_whitespace() || "|&;<>".contains(ch))
                .unwrap_or(rest.len());
            if end == 0 {
                (first.len_utf8(), palette::SPECIAL)
            } else {
                let token = &rest[..end];
                let color = if token.starts_with('-') {
                    palette::ACCENT
                } else if token.contains('$') {
                    palette::WARNING
                } else {
                    palette::TEXT
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

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;
    use super::*;
    use crate::app::View;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;
    use styra_server::event::AgentEvent;

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
        let mut app = testing::app("s1");
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
    fn pretty_preview_styles_message_markdown_and_raw_preview_keeps_it_literal() {
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "# Title\n- use `cargo test` for **all**".into(),
        });

        let pretty = preview_lines(&app);
        let text: String = pretty
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("Title"));
        assert!(!text.contains('#'));
        assert!(!text.contains("**"));
        assert!(!text.contains('`'));
        assert!(text.contains("• use cargo test for all"));
        assert!(pretty
            .iter()
            .flat_map(|line| &line.spans)
            .any(|span| span.style.fg == Some(palette::WARNING)));

        app.preview.toggle_mode();
        let raw: String = preview_lines(&app)
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect();
        assert!(raw.contains("# Title"));
        assert!(raw.contains("`cargo test`"));
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
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        // The preview must not depend on the entry being expanded in the list.
        assert!(!app.timeline.entries[0].expanded);
        assert!(!rendered(&app).contains("24 passed"));

        app.preview.toggle();
        let shown = rendered(&app);
        assert!(shown.contains("preview"));
        assert!(shown.contains("24 passed"));
    }

    #[test]
    fn command_mode_previews_the_newest_command_and_result_whatever_is_focused() {
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "all green".into(),
        });
        app.select_last();
        app.preview.toggle();

        // Focus is on the message, so that is what the preview shows.
        let shown = rendered(&app);
        assert!(shown.contains("all green"));
        assert!(!shown.contains("24 passed"));

        app.preview.toggle_target();
        let shown = rendered(&app);
        assert!(shown.contains("command"));
        assert!(shown.contains("cargo test"));
        assert!(shown.contains("24 passed"));
        // Still selecting the message; only the preview's target changed.
        assert_eq!(app.timeline.selected, app.timeline.entries.len() - 1);
    }

    #[test]
    fn command_preview_separates_the_command_from_its_output_with_a_blank_line() {
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.preview.toggle();

        let lines = preview_lines(&app);
        let output_row = lines
            .iter()
            .position(|line| {
                line.spans
                    .iter()
                    .any(|span| span.content.contains("24 passed"))
            })
            .expect("output line");
        let separator = &lines[output_row - 1];
        assert!(
            separator
                .spans
                .iter()
                .all(|span| span.content.trim().is_empty()),
            "expected a blank line directly above the output, found {separator:?}"
        );
    }

    #[test]
    fn command_mode_falls_back_to_the_selection_before_any_command_runs() {
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "all green".into(),
        });
        app.preview.toggle();
        app.preview.toggle_target();
        assert!(rendered(&app).contains("all green"));
    }

    #[test]
    fn file_diff_preview_toggles_between_minimal_and_raw_output() {
        let mut app = testing::app("s1");
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
        app.preview.toggle();

        let minimal = rendered(&app);
        assert!(minimal.contains("pretty"));
        assert!(minimal.contains("-old"));
        assert!(minimal.contains("+new"));
        assert!(!minimal.contains("diff --git"));
        assert!(!minimal.contains("@@ -1,3"));
        assert!(!minimal.contains("index 123"));
        assert!(!minimal.contains(" context"));

        app.preview.toggle_mode();
        let raw = rendered(&app);
        assert!(raw.contains("raw"));
        assert!(raw.contains("diff --git"));
        assert!(raw.contains("@@ -1,3"));
        assert!(raw.contains("index 123"));
        assert!(raw.contains(" context"));
    }

    #[test]
    fn compact_provider_diff_still_changes_visibly_between_modes() {
        let mut app = testing::app("s1");
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

        app.preview.toggle_mode();
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
            .any(|span| span.style.fg == Some(palette::ACCENT)));

        app.preview.toggle_mode();
        let raw = preview_lines(&app)
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(raw.contains("description"));
    }

    #[test]
    fn codex_bash_toggles_between_highlighted_command_and_wrapper() {
        let mut app = testing::app("s1");
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
            .any(|span| span.style.fg == Some(palette::ACCENT)));

        app.preview.toggle_mode();
        let raw = preview_lines(&app)
            .iter()
            .flat_map(|line| &line.spans)
            .map(|span| span.content.as_ref())
            .collect::<String>();
        assert!(raw.contains("/usr/bin/bash -lc"));
    }

    #[test]
    fn diff_preview_never_emits_literal_terminal_tabs() {
        let mut app = testing::app("s1");
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
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        assert!(!rendered(&app).contains("24 passed"));

        app.toggle_view(View::Preview);
        let shown = rendered(&app);
        // No chrome at all: no title bar, no message box, no footer hints —
        // just the entry's text, so it can be selected and copied cleanly.
        assert!(shown.contains("24 passed"));
        assert!(!shown.contains("codex"));
        assert!(!shown.contains("message"));
        assert!(!shown.contains("quit"));

        app.toggle_view(View::Preview);
        let restored = rendered(&app);
        assert!(restored.contains("codex"));
        assert!(restored.contains("Shell"));
    }

    #[test]
    fn fullscreen_preview_has_no_border_or_title() {
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "hello".into(),
        });
        app.toggle_view(View::Preview);

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
    fn preview_does_not_duplicate_the_current_content_of_a_changed_file() {
        let dir =
            std::env::temp_dir().join(format!("styra-preview-file-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "line one\nline two").unwrap();

        let mut app = testing::app("s1");
        app.workspace.enter(dir.clone());
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.preview.toggle();

        let screen = rendered(&app);
        assert!(screen.contains("notes.txt"));
        assert!(!screen.contains("line one"));
        assert!(!screen.contains("line two"));

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

        let mut app = testing::app("s1");
        app.workspace.enter(dir.clone());
        // A FileChanged entry exercises the ordinary preview detail body.
        app.push_event(AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["notes.txt".into()],
            diff: None,
            checkpoint: None,
            checkpoint_error: None,
        });
        app.preview.toggle();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        // The preview occupies the right ~40% of the frame (the list's own
        // selection highlight lives to the left of that and is unaffected).
        // `Style::default()` renders with the palette's reset color, not `None`, so
        // check for the specific highlight color rather than any `Some` bg.
        let preview_columns = 50..buffer.area.width;
        let has_highlight = preview_columns
            .flat_map(|x| (0..buffer.area.height).map(move |y| (x, y)))
            .any(|(x, y)| {
                buffer.cell((x, y)).unwrap().style().bg == Some(palette::SELECTION_BACKGROUND)
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
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "hello".into(),
        });
        app.preview.toggle();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let (x, y) = find_column(&buffer, "preview");
        let cell = buffer.cell((x, y)).unwrap();
        assert_ne!(cell.style().fg, Some(palette::INACTIVE));
    }

    #[test]
    fn preview_wrapped_agent_messages_keep_a_hanging_indent() {
        // The list panel already hangs a wrapped continuation under the
        // `«`/`»` marker instead of jumping to the left edge; the preview
        // panel must wrap the same way rather than leaning on the widget's
        // own left-flush word wrap.
        let mut app = testing::app("s1");
        app.push_event(AgentEvent::AgentMessage {
            text: "one two three four five six seven eight nine ten eleven twelve".into(),
        });
        app.preview.toggle();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let full_area = ratatui::layout::Rect::new(0, 0, 80, 20);
        let chunks = ratatui::layout::Layout::default()
            .direction(ratatui::layout::Direction::Horizontal)
            .constraints([
                ratatui::layout::Constraint::Percentage(60),
                ratatui::layout::Constraint::Percentage(40),
            ])
            .split(full_area);
        let preview_area = chunks[1];
        let left = preview_area.x + 1;
        let right = preview_area.x + preview_area.width - 1;

        let rows: Vec<String> = (preview_area.y + 1..preview_area.y + preview_area.height - 1)
            .map(|y| {
                (left..right)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .filter(|row| row.contains("one") || row.contains("four") || row.contains("seven"))
            .collect();

        assert!(rows.len() >= 2, "{rows:?}");
        assert!(
            rows.iter().all(|row| row.starts_with(DETAIL_INDENT)),
            "wrapped preview rows must all share the detail body's hanging indent: {rows:?}"
        );
    }
}
