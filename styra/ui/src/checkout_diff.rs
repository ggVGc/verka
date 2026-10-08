//! An interaction's checkout diffed against the commit its branch was made
//! at, shown inside Styra rather than in a window of its own.
//!
//! The diff alone does not say what it is a diff *of*, so a header above it
//! names both sides — the branch being worked on and the branch and commit it
//! started from — and the worktree they are read in. The header stays put
//! while the diff below it scrolls.

use crate::chrome::{panel_block, PanelChrome};
use crate::diff::{diff_block_lines, fill_changed_rows};
use crate::theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub struct CheckoutDiffView<'a> {
    pub chrome: PanelChrome,
    /// Root of the working tree that was diffed.
    pub worktree: &'a str,
    /// The branch checked out there. `None` when its head is detached.
    pub branch: Option<&'a str>,
    /// The branch the checkout's branch was made from. `None` when the
    /// repository's head was detached at the time.
    pub base_branch: Option<&'a str>,
    /// The commit the checkout's branch was made at: the side the diff is
    /// measured from.
    pub base_commit: &'a str,
    /// `git diff`'s output, or why there is none.
    pub diff: Result<&'a str, &'a str>,
    pub requested_scroll: u16,
}

/// How much of a commit id the header shows: enough to tell commits apart in
/// any repository an operator is likely to have, short enough to read.
const SHORT_COMMIT: usize = 10;

/// Draw the view, and return how far its diff can scroll.
pub fn render(frame: &mut Frame, view: &CheckoutDiffView<'_>, area: Rect) -> u16 {
    let block = panel_block(&view.chrome);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let header = header_lines(view);
    let [top, body] = Layout::vertical([
        // One more than the header for the rule beneath it.
        Constraint::Length(header.len() as u16 + 1),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(
        Paragraph::new(header).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme::INACTIVE)),
        ),
        top,
    );
    let mut lines = match view.diff {
        Ok(text) if text.trim().is_empty() => {
            vec![muted("  no changes since the branch was made")]
        }
        Ok(text) => diff_block_lines(text, None, false, ""),
        Err(error) => vec![Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(theme::ERROR),
        ))],
    };
    fill_changed_rows(&mut lines, usize::from(body.width));
    let limit = (lines.len() as u16).saturating_sub(body.height);
    frame.render_widget(
        Paragraph::new(lines).scroll((view.requested_scroll.min(limit), 0)),
        body,
    );
    limit
}

/// What is compared with what, where, and how much changed.
fn header_lines(view: &CheckoutDiffView<'_>) -> Vec<Line<'static>> {
    let strong = Style::default()
        .fg(theme::TEXT)
        .add_modifier(Modifier::BOLD);
    let commit = view
        .base_commit
        .get(..SHORT_COMMIT)
        .unwrap_or(view.base_commit);
    let mut comparison = vec![
        Span::styled(
            view.branch.unwrap_or("detached head").to_owned(),
            strong.fg(theme::WORKSPACE_NAME),
        ),
        Span::styled("  against  ", Style::default().fg(theme::MUTED_TEXT)),
    ];
    if let Some(base) = view.base_branch {
        comparison.push(Span::styled(base.to_owned(), strong));
        comparison.push(Span::styled(" at ", Style::default().fg(theme::MUTED_TEXT)));
    }
    comparison.push(Span::styled(
        commit.to_owned(),
        Style::default().fg(theme::ACCENT),
    ));
    comparison.push(Span::styled(
        "  (where the branch started)",
        Style::default().fg(theme::MUTED_TEXT),
    ));
    let mut lines = vec![
        Line::from(comparison),
        Line::from(vec![
            Span::styled(
                view.worktree.to_owned(),
                Style::default().fg(theme::DIRECTORY_NAME),
            ),
            Span::styled(
                "  · working tree, uncommitted changes included",
                Style::default().fg(theme::MUTED_TEXT),
            ),
        ]),
    ];
    if let Ok(text) = view.diff {
        lines.push(stat_line(text));
    }
    lines
}

/// Files changed and lines added and removed, counted from the diff itself so
/// the header costs no second `git` run.
fn stat_line(text: &str) -> Line<'static> {
    let (mut files, mut added, mut removed) = (0, 0, 0);
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            files += 1;
        } else if !line.starts_with("+++ ") && !line.starts_with("--- ") {
            match line.as_bytes().first() {
                Some(b'+') => added += 1,
                Some(b'-') => removed += 1,
                _ => {}
            }
        }
    }
    let noun = if files == 1 { "file" } else { "files" };
    Line::from(vec![
        Span::styled(
            format!("{files} {noun} changed  "),
            Style::default().fg(theme::TEXT),
        ),
        Span::styled(format!("+{added}"), Style::default().fg(theme::SUCCESS)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(theme::ERROR)),
    ])
}

fn muted(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(theme::MUTED_TEXT),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::StatusTone;
    use ratatui::{backend::TestBackend, Terminal};

    const DIFF: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 fn a() {}
-fn b() {}
+fn c() {}
+fn d() {}
";

    fn chrome() -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: None,
            worktree: None,
            agent: "codex".into(),
            model: "gpt".into(),
            model_reported: true,
            effort: None,
            effort_reported: false,
            status: "idle".into(),
            status_tone: StatusTone::Idle,
            elapsed: None,
            suffix: Some("diff".into()),
            session: None,
        }
    }

    fn screen(diff: Result<&str, &str>, base_branch: Option<&str>, scroll: u16) -> (String, u16) {
        let (width, height) = (80, 16);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut limit = 0;
        terminal
            .draw(|frame| {
                limit = render(
                    frame,
                    &CheckoutDiffView {
                        chrome: chrome(),
                        worktree: "/state/worktrees/styra-7",
                        branch: Some("styra/rename"),
                        base_branch,
                        base_commit: "4bf5c35d0123456789abcdef",
                        diff,
                        requested_scroll: scroll,
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        (output, limit)
    }

    #[test]
    fn the_header_names_both_sides_and_where_they_were_read() {
        let (output, _) = screen(Ok(DIFF), Some("main"), 0);
        assert!(
            output.contains("styra/rename  against  main at 4bf5c35d01  (where"),
            "{output}"
        );
        assert!(output.contains("/state/worktrees/styra-7"), "{output}");
        assert!(output.contains("1 file changed  +2 -1"), "{output}");
        assert!(output.contains("fn c() {}"), "{output}");
    }

    /// The hint fills the row out to the panel's edge, not just the text.
    #[test]
    fn a_changed_row_is_tinted_across_the_panel() {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        chrome: chrome(),
                        worktree: "/w",
                        branch: None,
                        base_branch: None,
                        base_commit: "4bf5c35d",
                        diff: Ok(DIFF),
                        requested_scroll: 0,
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = (0..16u16)
            .find(|&y| {
                (0..80u16)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("fn c()")
            })
            .unwrap();
        assert_eq!(buffer[(70, row)].bg, theme::DIFF_ADDED_BACKGROUND);
    }

    #[test]
    fn a_base_made_from_a_detached_head_is_named_by_its_commit_alone() {
        let (output, _) = screen(Ok(DIFF), None, 0);
        assert!(
            output.contains("styra/rename  against  4bf5c35d01"),
            "{output}"
        );
    }

    #[test]
    fn the_diff_scrolls_beneath_a_header_that_stays() {
        let long = (0..20).fold(DIFF.to_owned(), |text, n| {
            text + &format!("+fn x{n}() {{}}\n")
        });
        let (output, limit) = screen(Ok(&long), Some("main"), u16::MAX);
        assert!(limit > 0);
        assert!(output.contains("against"), "{output}");
        assert!(output.contains("fn x19() {}"), "{output}");
        assert!(!output.contains("diff --git"), "{output}");
    }

    #[test]
    fn no_changes_and_failures_say_so() {
        let (empty, limit) = screen(Ok(""), Some("main"), 0);
        assert_eq!(limit, 0);
        assert!(
            empty.contains("no changes since the branch was made"),
            "{empty}"
        );
        assert!(empty.contains("0 files changed"), "{empty}");

        let (failed, _) = screen(Err("git diff failed: bad revision"), Some("main"), 0);
        assert!(failed.contains("git diff failed: bad revision"), "{failed}");
        assert!(!failed.contains("files changed"), "{failed}");
    }
}
