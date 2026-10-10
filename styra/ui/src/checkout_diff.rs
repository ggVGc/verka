//! An interaction's checkout diffed against the commit its branch was made
//! at, shown inside Styra rather than in a window of its own.
//!
//! The diff alone does not say what it is a diff *of*, so a header above it
//! names both sides — the branch being worked on and the branch and commit it
//! started from — and the worktree they are read in. The header stays put
//! while the diff below it scrolls.

use crate::chrome::{panel_block, PanelChrome};
use crate::diff::{diff_block_lines, fill_changed_rows};
use crate::preview::PreviewFeedback;
use crate::theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
    pub per_file: bool,
    pub hide_removed: bool,
    pub selected_file: usize,
    pub search: crate::search::SearchView<'a>,
}

/// A complete patch for one changed file, including metadata for renames,
/// binary changes, deletions, and changes of mode that have no hunks.
pub struct FileDiff<'a> {
    pub path: &'a str,
    pub patch: &'a str,
}

/// Split only on Git's file headers, retaining each patch verbatim.
pub fn file_diffs(text: &str) -> Vec<FileDiff<'_>> {
    let mut starts = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") {
            starts.push(offset);
        }
        offset += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let end = starts.get(index + 1).copied().unwrap_or(text.len());
            let patch = &text[start..end];
            let header = patch.lines().next().unwrap_or_default();
            // The destination marker handles spaces in paths. A deleted file
            // uses its source; renames without hunks use their destination.
            let marker_path = |marker: &str| {
                patch
                    .lines()
                    .take_while(|line| !line.starts_with("@@"))
                    .find_map(|line| {
                        line.strip_prefix(marker)
                            .filter(|path| *path != "/dev/null")
                    })
                    .map(|path| {
                        let path = path.trim_matches('"');
                        path.strip_prefix("b/")
                            .or_else(|| path.strip_prefix("a/"))
                            .unwrap_or(path)
                    })
            };
            let path = marker_path("+++ ")
                .or_else(|| {
                    patch
                        .lines()
                        .find_map(|line| line.strip_prefix("rename to "))
                })
                .or_else(|| marker_path("--- "))
                .unwrap_or_else(|| {
                    header
                        .rsplit_once(" b/")
                        .or_else(|| header.rsplit_once(" \"b/"))
                        .map_or(header, |(_, path)| path.trim_end_matches('"'))
                });
            FileDiff { path, patch }
        })
        .collect()
}

/// File-name filtering starts with the first character and ignores case.
pub fn filtered_file_diffs<'a>(text: &'a str, query: Option<&str>) -> Vec<(usize, FileDiff<'a>)> {
    let query = query.unwrap_or_default().to_lowercase();
    let mut files: Vec<_> = file_diffs(text)
        .into_iter()
        .enumerate()
        .filter(|(_, file)| file.path.to_lowercase().contains(&query))
        .collect();
    // Compare components so a directory's descendants stay together even
    // when a sibling filename shares its prefix (src/a.rs and src/a/b.rs).
    files.sort_by(|(_, a), (_, b)| a.path.split('/').cmp(b.path.split('/')));
    files
}

/// Expanded directory rows and file leaves. Only leaves have a file index,
/// so keyboard navigation continues to select patches rather than folders.
fn file_tree_rows<'a>(files: &[(usize, FileDiff<'a>)]) -> Vec<(Option<usize>, Line<'a>)> {
    let mut rows = Vec::new();
    let mut previous_dirs = Vec::new();
    for (index, file) in files {
        let components: Vec<_> = file.path.split('/').collect();
        let (name, dirs) = components.split_last().unwrap();
        let shared = dirs
            .iter()
            .zip(&previous_dirs)
            .take_while(|(a, b)| a == b)
            .count();
        for (depth, directory) in dirs.iter().enumerate().skip(shared) {
            rows.push((
                None,
                Line::from(Span::styled(
                    format!("{}▾ {directory}/", "  ".repeat(depth)),
                    Style::default().fg(theme::MUTED_TEXT),
                )),
            ));
        }
        rows.push((
            Some(*index),
            Line::from(format!("{}{name}", "  ".repeat(dirs.len()))),
        ));
        previous_dirs = dirs.to_vec();
    }
    rows
}

/// How much of a commit id the header shows: enough to tell commits apart in
/// any repository an operator is likely to have, short enough to read.
const SHORT_COMMIT: usize = 10;

/// Draw the view, and return how far its diff can scroll.
pub fn render(frame: &mut Frame, view: &CheckoutDiffView<'_>, area: Rect) -> PreviewFeedback {
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
    let files = view
        .diff
        .ok()
        .map(|text| filtered_file_diffs(text, view.search.query))
        .unwrap_or_default();
    let body = if view.per_file {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
                .areas(body);
        let rows = file_tree_rows(&files);
        let selected = rows
            .iter()
            .position(|(index, _)| *index == Some(view.selected_file));
        let items = rows.into_iter().map(|(_, line)| ListItem::new(line));
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(theme::INACTIVE)),
            )
            .highlight_style(
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        frame.render_stateful_widget(
            list,
            left,
            &mut ListState::default().with_selected(selected),
        );
        right
    } else {
        body
    };
    let shown = if view.per_file {
        files
            .iter()
            .find(|(index, _)| *index == view.selected_file)
            .or_else(|| files.first())
            .map(|(_, file)| Ok(file.patch))
            .unwrap_or_else(|| {
                if view.search.query.is_some_and(|query| !query.is_empty()) && view.diff.is_ok() {
                    Err("no matching files")
                } else {
                    view.diff
                }
            })
    } else {
        view.diff
    };
    let mut lines = match shown {
        Ok(text) if text.trim().is_empty() => {
            vec![muted("  no changes since the branch was made")]
        }
        Ok(text) => patch_lines(text, view.hide_removed),
        Err(error) => vec![Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(theme::ERROR),
        ))],
    };
    fill_changed_rows(&mut lines, usize::from(body.width));
    let limit = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(body.height);
    frame.render_widget(
        Paragraph::new(lines).scroll((view.requested_scroll.min(limit), 0)),
        body,
    );
    PreviewFeedback {
        limit,
        effective_scroll: view.requested_scroll.min(limit),
        viewport: body.height,
    }
}

/// Filter after rendering so line numbers and syntax highlighting still
/// follow the complete patch, including lines hidden from the view.
fn patch_lines(text: &str, hide_removed: bool) -> Vec<Line<'static>> {
    let mut lines = diff_block_lines(text, None, false, "");
    if hide_removed {
        lines.retain(|line| line.style.bg != Some(theme::DIFF_REMOVED_BACKGROUND));
    }
    lines
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
    let navigation = if view.per_file {
        "f: combined diff · /: search · j/k: files · J/K: scroll 10 · PgUp/PgDn: half-screen"
    } else {
        "f: per-file diffs · j/k: scroll · J/K: scroll 10 · PgUp/PgDn: half-screen"
    };
    lines.push(muted(navigation));
    lines.push(muted(if view.hide_removed {
        "h: show removed lines · removed lines hidden"
    } else {
        "h: hide removed lines"
    }));
    if view.per_file {
        if let Some(query) = view.search.query {
            lines.push(muted(&format!(
                "/{query}  · {}",
                if view.search.typing {
                    "Enter: keep filter · Esc: clear"
                } else {
                    "Esc: clear filter"
                }
            )));
        }
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

    #[test]
    fn hiding_removals_keeps_added_indicators_and_original_line_numbers() {
        let lines = patch_lines(DIFF, true);
        let text = lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("fn b()"), "{text}");
        assert!(text.contains("1  fn a()"), "{text}");
        assert!(text.contains("2 +fn c()"), "{text}");
        assert!(text.contains("3 +fn d()"), "{text}");
        assert!(text.contains("--- a/src/a.rs"), "{text}");
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.style.bg == Some(theme::DIFF_ADDED_BACKGROUND))
                .count(),
            2
        );
        assert!(patch_lines(DIFF, false)
            .iter()
            .any(|line| line.style.bg == Some(theme::DIFF_REMOVED_BACKGROUND)));
    }

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
        file_screen(diff, base_branch, scroll, false, 0).0
    }

    fn file_screen(
        diff: Result<&str, &str>,
        base_branch: Option<&str>,
        scroll: u16,
        per_file: bool,
        selected_file: usize,
    ) -> ((String, u16), u16) {
        filtered_screen(
            diff,
            base_branch,
            scroll,
            per_file,
            selected_file,
            Default::default(),
        )
    }

    fn filtered_screen(
        diff: Result<&str, &str>,
        base_branch: Option<&str>,
        scroll: u16,
        per_file: bool,
        selected_file: usize,
        search: crate::search::SearchView<'_>,
    ) -> ((String, u16), u16) {
        let (width, height) = (80, 16);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut measured = PreviewFeedback::default();
        terminal
            .draw(|frame| {
                measured = render(
                    frame,
                    &CheckoutDiffView {
                        chrome: chrome(),
                        worktree: "/state/worktrees/styra-7",
                        branch: Some("styra/rename"),
                        base_branch,
                        base_commit: "4bf5c35d0123456789abcdef",
                        diff,
                        requested_scroll: scroll,
                        per_file,
                        hide_removed: false,
                        selected_file,
                        search,
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
        ((output, measured.limit), measured.viewport)
    }

    #[test]
    fn file_search_filters_names_and_the_selected_patch_together() {
        let text =
            format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old second\n+new second\n");
        let files = filtered_file_diffs(&text, Some("B.T"));
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, 1);
        assert_eq!(files[0].1.path, "b.txt");
        let ((output, _), _) = filtered_screen(
            Ok(&text),
            None,
            0,
            true,
            1,
            crate::search::SearchView {
                query: Some("B.T"),
                typing: true,
            },
        );
        assert!(output.contains("/B.T"), "{output}");
        assert!(output.contains("Enter: keep filter"), "{output}");
        assert!(output.contains("› b.txt"), "{output}");
        assert!(output.contains("new second"), "{output}");
        assert!(!output.contains("src/a.rs"), "{output}");
        assert!(!output.contains("fn c()"), "{output}");

        let ((output, limit), _) = filtered_screen(
            Ok(&text),
            None,
            0,
            true,
            1,
            crate::search::SearchView {
                query: Some("missing"),
                typing: false,
            },
        );
        assert!(output.contains("no matching files"), "{output}");
        assert!(!output.contains("new second"), "{output}");
        assert!(!output.contains("fn c()"), "{output}");
        assert_eq!(limit, 0);
    }

    #[test]
    fn file_sections_include_deleted_renamed_binary_and_mode_only_changes() {
        let patches = [
            DIFF.to_owned(),
            "diff --git a/gone.txt b/gone.txt\n--- a/gone.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-gone\n".into(),
            "diff --git a/old name b/a/new name\nsimilarity index 100%\nrename from old name\nrename to a/new name\n".into(),
            "diff --git a/a/image.png b/a/image.png\nBinary files a/a/image.png and b/a/image.png differ\n".into(),
            "diff --git a/script b/script\nold mode 100644\nnew mode 100755\n".into(),
            "diff --git \"a/tab\\tname\" \"b/tab\\tname\"\n--- \"a/tab\\tname\"\n+++ \"b/tab\\tname\"\n@@ -1 +1 @@\n-old\n+new\n".into(),
        ];
        let text = patches.concat();
        let files = file_diffs(&text);
        assert_eq!(
            files.iter().map(|file| file.path).collect::<Vec<_>>(),
            [
                "src/a.rs",
                "gone.txt",
                "a/new name",
                "a/image.png",
                "script",
                "tab\\tname"
            ]
        );
        for (file, patch) in files.iter().zip(&patches) {
            assert_eq!(file.patch, patch);
        }
        assert!(file_diffs("").is_empty());
    }

    #[test]
    fn per_file_view_keeps_files_on_the_left_and_only_the_selected_patch_on_the_right() {
        let text =
            format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old second\n+new second\n");
        let ((output, _), viewport) = file_screen(Ok(&text), Some("main"), 0, true, 1);
        let rows = output.lines().collect::<Vec<_>>();
        assert!(
            rows.iter().any(|row| row
                .chars()
                .take(24)
                .collect::<String>()
                .contains("▾ src/")),
            "{output}"
        );
        assert!(
            rows.iter().any(|row| row
                .chars()
                .take(24)
                .collect::<String>()
                .contains("  a.rs")),
            "{output}"
        );
        assert!(
            rows.iter()
                .any(|row| row.chars().take(24).collect::<String>().contains("› b.txt")),
            "{output}"
        );
        assert!(
            rows.iter().any(|row| row
                .chars()
                .skip(24)
                .collect::<String>()
                .contains("new second")),
            "{output}"
        );
        assert!(!output.contains("fn c()"), "{output}");
        assert!(output.contains("2 files changed"), "{output}");
        assert_eq!(viewport, 8);
    }

    #[test]
    fn selection_remains_visible_in_a_long_file_tree() {
        let text = (0..30)
            .map(|n| format!("diff --git a/src/dir{n}/file{n} b/src/dir{n}/file{n}\n@@ -1 +1 @@\n-old{n}\n+new{n}\n"))
            .collect::<String>();
        let ((output, _), _) = file_screen(Ok(&text), None, 0, true, 29);
        assert!(output.contains("›     file29"), "{output}");
        assert!(output.contains("new29"), "{output}");
        assert!(!output.contains("new0"), "{output}");
    }

    #[test]
    fn tree_groups_shared_directories_and_keeps_original_patch_indices_when_filtered() {
        let text = ["src/a.rs", "src/a/z.rs", "README.md", "src/a/b.rs", "src/z.rs"]
            .iter()
            .map(|path| format!("diff --git a/{path} b/{path}\n"))
            .collect::<String>();
        let files = filtered_file_diffs(&text, None);
        let rows = file_tree_rows(&files);
        let displayed: Vec<_> = rows
            .iter()
            .map(|(index, line)| (*index, line.to_string()))
            .collect();
        assert_eq!(displayed, vec![
            (Some(2), "README.md".into()),
            (None, "▾ src/".into()),
            (None, "  ▾ a/".into()),
            (Some(3), "    b.rs".into()),
            (Some(1), "    z.rs".into()),
            (Some(0), "  a.rs".into()),
            (Some(4), "  z.rs".into()),
        ]);
        let files = filtered_file_diffs(&text, Some("src/a/"));
        assert_eq!(files.iter().map(|(index, _)| *index).collect::<Vec<_>>(), [3, 1]);
        assert_eq!(file_tree_rows(&files).len(), 4);
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
                        per_file: false,
                        hide_removed: false,
                        selected_file: 0,
                        search: Default::default(),
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
