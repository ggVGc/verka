//! A working tree diffed against a commit, drawn into whatever area the host
//! gives it: a panel inside Styra, or the whole terminal for `styra-diff`.
//!
//! The diff alone does not say what it is a diff *of*, so a header above it
//! names both sides — the branch being worked on and the branch and commit it
//! started from — and the worktree they are read in. The header stays put
//! while the diff below it scrolls.

use crate::files::{filtered_file_diffs, FileChange, FileDiff};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::path::Path;
use styra_ui::application::{Panel, PanelFeedback};
use styra_ui::diff::{diff_block_lines, diff_block_lines_fading, fill_changed_rows, CONTEXT_FADE};
use styra_ui::preview::PreviewFeedback;
use styra_ui::search::SearchView;
use styra_ui::theme;

pub struct CheckoutDiffView<'a> {
    /// The frame drawn around the view: Styra's panel chrome, or a plain
    /// border when the view runs on its own.
    pub block: Block<'a>,
    /// Root of the working tree that was diffed.
    pub worktree: &'a Path,
    /// The branch checked out there. `None` when its head is detached.
    pub branch: Option<&'a str>,
    /// The branch the checkout's branch was made from. `None` when the
    /// repository's head was detached at the time.
    pub base_branch: Option<&'a str>,
    /// The commit the diff is measured from.
    pub base_commit: &'a str,
    /// The branches and tags at that commit, as `git log --decorate` names
    /// them. The branches the header already names, checked out or base,
    /// are not repeated.
    pub base_names: &'a [String],
    /// Whether that commit is where the checked-out branch started, which the
    /// header says, rather than just a commit to compare with.
    pub branch_point: bool,
    /// `git diff`'s output, or why there is none.
    pub diff: Result<&'a str, &'a str>,
    pub requested_scroll: u16,
    pub per_file: bool,
    pub hide_removed: bool,
    /// Whether the file tree folds directories with no files of their own
    /// and a single subdirectory onto one row.
    pub compact_dirs: bool,
    pub tint: Tint,
    pub selected_file: usize,
    /// First visible row in the file tree, retained between frames.
    pub file_list_offset: usize,
    pub search: SearchView<'a>,
}

impl<'a> CheckoutDiffView<'a> {
    /// The view before any diff has been read into it, saying so.
    pub fn unread(block: Block<'a>) -> Self {
        Self {
            block,
            worktree: Path::new(""),
            branch: None,
            base_branch: None,
            base_commit: "",
            base_names: &[],
            branch_point: false,
            diff: Err("no diff read yet"),
            requested_scroll: 0,
            per_file: false,
            hide_removed: false,
            compact_dirs: true,
            tint: Tint::Full,
            selected_file: 0,
            file_list_offset: 0,
            search: SearchView::default(),
        }
    }
}

/// How far unchanged rows, and the diff's other metadata, are faded toward
/// the background. Added and removed rows keep their bands throughout.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tint {
    /// Faded as Styra draws diffs everywhere else.
    #[default]
    Full,
    /// Half as faded.
    Half,
}

impl Tint {
    /// The other level: full, half, and back.
    pub fn next(self) -> Self {
        match self {
            Self::Full => Self::Half,
            Self::Half => Self::Full,
        }
    }

    /// Tenths of the way unchanged rows are faded toward the background.
    fn fade(self) -> u16 {
        match self {
            Self::Full => CONTEXT_FADE,
            Self::Half => CONTEXT_FADE / 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CheckoutDiffFeedback {
    pub scroll: PreviewFeedback,
    pub file_list_offset: usize,
}

/// Styra draws the view as its main panel, which `styra-ui` cannot name.
impl Panel for CheckoutDiffView<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) -> PanelFeedback {
        let measured = render(frame, self, area);
        PanelFeedback {
            scroll: measured.scroll,
            list_offset: Some(measured.file_list_offset),
        }
    }
}

/// One directory of the file tree: its files and subdirectories, in the
/// order the sorted paths gave them.
#[derive(Default)]
struct Directory<'a> {
    children: Vec<Node<'a>>,
}

enum Node<'a> {
    Directory(&'a str, Directory<'a>),
    /// A changed file: its patch's index, name, and change kind.
    File(usize, &'a str, FileChange),
}

impl<'a> Directory<'a> {
    /// The tree of `files`, which are sorted by path component, so a
    /// directory's descendants arrive together and each one need only be
    /// compared with the directory last opened beside it.
    fn of(files: &[(usize, FileDiff<'a>)]) -> Self {
        let mut root = Self::default();
        for (index, file) in files {
            let components: Vec<_> = file.path.split('/').collect();
            let (name, dirs) = components.split_last().unwrap();
            let mut directory = &mut root;
            for &dir in dirs {
                let open = matches!(
                    directory.children.last(),
                    Some(Node::Directory(last, _)) if *last == dir
                );
                if !open {
                    directory
                        .children
                        .push(Node::Directory(dir, Self::default()));
                }
                let Some(Node::Directory(_, child)) = directory.children.last_mut() else {
                    unreachable!("a directory was just found or opened last");
                };
                directory = child;
            }
            directory
                .children
                .push(Node::File(*index, name, file.change()));
        }
        root
    }

    fn rows(&self, depth: usize, compact: bool, rows: &mut Vec<(Option<usize>, Line<'a>)>) {
        let indent = "  ".repeat(depth);
        for child in &self.children {
            match child {
                Node::File(index, name, change) => {
                    let color = match change {
                        FileChange::Added | FileChange::Copied => theme::SUCCESS,
                        FileChange::Deleted => theme::ERROR,
                        FileChange::Moved => theme::DIRECTORY_NAME,
                        _ => theme::WARNING,
                    };
                    rows.push((
                        Some(*index),
                        Line::from(vec![
                            Span::raw(format!("{indent}{name}")),
                            Span::styled(
                                format!(" [{}]", change.marker()),
                                Style::default().fg(color),
                            ),
                        ]),
                    ));
                }
                Node::Directory(name, directory) => {
                    let mut path = (*name).to_owned();
                    let mut directory = directory;
                    // A directory holding nothing but one other directory says
                    // nothing on a row of its own: name both on one.
                    while let (true, [Node::Directory(name, only)]) =
                        (compact, directory.children.as_slice())
                    {
                        path = format!("{path}/{name}");
                        directory = only;
                    }
                    rows.push((
                        None,
                        Line::from(Span::styled(
                            format!("{indent}▾ {path}/"),
                            Style::default().fg(theme::DIRECTORY_NAME),
                        )),
                    ));
                    directory.rows(depth + 1, compact, rows);
                }
            }
        }
    }
}

/// Expanded directory rows and file leaves. Only leaves have a file index,
/// so keyboard navigation continues to select patches rather than folders.
/// `compact` folds a chain of directories that hold no files of their own,
/// each with just one subdirectory, onto a single row.
fn file_tree_rows<'a>(
    files: &[(usize, FileDiff<'a>)],
    compact: bool,
) -> Vec<(Option<usize>, Line<'a>)> {
    let mut rows = Vec::new();
    Directory::of(files).rows(0, compact, &mut rows);
    rows
}

/// Marks the selected file in the tree, and is room every row leaves for it.
const SELECTION_MARKER: &str = "› ";

/// Blank columns kept between the tree's longest row and its border.
const FILE_TREE_PADDING: u16 = 2;

/// As wide as the tree's longest row needs, and no wider, so the patch beside
/// it gets the rest — but never more than half of `available`, so one deep
/// path cannot squeeze the patch out of view.
fn file_tree_width(rows: &[(Option<usize>, Line<'_>)], available: u16) -> u16 {
    let longest = rows.iter().map(|(_, line)| line.width()).max().unwrap_or(0);
    let marker = Line::from(SELECTION_MARKER).width();
    // One more for the border on the right.
    let wanted = u16::try_from(marker + longest)
        .unwrap_or(u16::MAX)
        .saturating_add(FILE_TREE_PADDING + 1);
    wanted.min(available / 2)
}

/// How much of a commit id the header shows: enough to tell commits apart in
/// any repository an operator is likely to have, short enough to read.
const SHORT_COMMIT: usize = 10;

/// Draw the view, and return how far its diff can scroll.
pub fn render(frame: &mut Frame, view: &CheckoutDiffView<'_>, area: Rect) -> CheckoutDiffFeedback {
    let block = view.block.clone();
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
    let mut file_list_offset = view.file_list_offset;
    let body = if view.per_file {
        let mut rows = file_tree_rows(&files, view.compact_dirs);
        let [left, right] = Layout::horizontal([
            Constraint::Length(file_tree_width(&rows, body.width)),
            Constraint::Min(0),
        ])
        .areas(body);
        let selected = rows
            .iter()
            .position(|(index, _)| *index == Some(view.selected_file));
        // Only the name: its change marker keeps its own color.
        if let Some(name) = selected.and_then(|row| rows[row].1.spans.first_mut()) {
            name.style = name.style.fg(theme::SELECTED_FILE_NAME);
        }
        let items = rows.into_iter().map(|(_, line)| ListItem::new(line));
        let list = List::new(items)
            .scroll_padding(5)
            .block(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(theme::INACTIVE)),
            )
            // A soft band rather than a color of its own, so the selected
            // row keeps its change marker's color and stands out by its bar.
            .highlight_style(
                Style::default()
                    .bg(theme::SELECTION_BACKGROUND)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(SELECTION_MARKER);
        let mut state = ListState::default()
            .with_selected(selected)
            .with_offset(file_list_offset);
        frame.render_stateful_widget(list, left, &mut state);
        file_list_offset = state.offset();
        right
    } else {
        body
    };
    let selected_file = view
        .per_file
        .then(|| {
            files
                .iter()
                .find(|(index, _)| *index == view.selected_file)
                .or_else(|| files.first())
                .map(|(_, file)| file)
        })
        .flatten();
    let body = if let Some(file) = selected_file {
        let [info, patch] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        frame.render_widget(Paragraph::new(file_info(file)), info);
        patch
    } else {
        body
    };
    let shown = if view.per_file {
        selected_file.map(|file| Ok(file.patch)).unwrap_or_else(|| {
            if view.search.query.is_some_and(|query| !query.is_empty()) && view.diff.is_ok() {
                Err("no matching files")
            } else {
                view.diff
            }
        })
    } else {
        view.diff
    };
    let width = usize::from(body.width);
    let mut lines = match shown {
        Ok(text) if text.trim().is_empty() => vec![muted(if view.branch_point {
            "  no changes since the branch was made"
        } else {
            "  no changes"
        })],
        Ok(text) => patch_lines(text, view.hide_removed, view.tint, width),
        Err(error) => vec![Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(theme::ERROR),
        ))],
    };
    fill_changed_rows(&mut lines, width);
    let limit = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(body.height);
    frame.render_widget(
        Paragraph::new(lines).scroll((view.requested_scroll.min(limit), 0)),
        body,
    );
    CheckoutDiffFeedback {
        scroll: PreviewFeedback {
            limit,
            effective_scroll: view.requested_scroll.min(limit),
            viewport: body.height,
        },
        file_list_offset,
    }
}

/// Keep the selected file's explanation visible while its patch scrolls.
fn file_info(file: &FileDiff<'_>) -> Line<'static> {
    let source = file
        .patch
        .lines()
        .take_while(|line| !line.starts_with("@@"))
        .find_map(|line| {
            line.strip_prefix("rename from ")
                .or_else(|| line.strip_prefix("copy from "))
        });
    let mut description = file.change().label().to_owned();
    if let Some(source) = source {
        description.push_str(&format!(" from {source}"));
    }
    Line::from(vec![
        Span::styled(
            format!(" {}", file.path),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" · {description}"),
            Style::default().fg(theme::ADDITIONAL_INFO),
        ),
    ])
}

/// What one row of the view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    /// A line of the diff, by its place among all the diff's lines.
    Line(usize),
    /// The rule along the top of a file's header, after the file before it.
    RuleAbove,
    /// The rule along the bottom of a file's header.
    RuleBelow,
}

/// The rows `text` is drawn as: a row for each of its lines, less the
/// removed ones when they are hidden, and each file's header set between two
/// rules — the one above only when something is drawn above it.
pub(crate) fn rows(text: &str, hide_removed: bool) -> Vec<Row> {
    let removed = if hide_removed {
        removed_rows(text)
    } else {
        Vec::new()
    };
    plan(text, &removed)
}

/// [`rows`], given which lines are `removed` and to be left out.
fn plan(text: &str, removed: &[bool]) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if removed.get(index).copied().unwrap_or(false) {
            continue;
        }
        let header = is_file_header(line);
        if header && !rows.is_empty() {
            rows.push(Row::RuleAbove);
        }
        rows.push(Row::Line(index));
        if header {
            rows.push(Row::RuleBelow);
        }
    }
    rows
}

fn is_file_header(line: &str) -> bool {
    line.starts_with("diff --git ")
}

/// `text` drawn as its [`rows`], `width` wide. Removed lines are left out
/// after rendering, so line numbers and syntax highlighting still follow the
/// complete patch.
fn patch_lines(text: &str, hide_removed: bool, tint: Tint, width: usize) -> Vec<Line<'static>> {
    // The diff is drawn a row for each of its lines.
    let mut rendered: Vec<_> = diff_block_lines_fading(text, None, false, "", tint.fade())
        .into_iter()
        .map(Some)
        .collect();
    let removed: Vec<bool> = if hide_removed {
        rendered
            .iter()
            .map(|line| line.as_ref().is_some_and(is_removed))
            .collect()
    } else {
        Vec::new()
    };
    let headers: Vec<bool> = text.lines().map(is_file_header).collect();
    plan(text, &removed)
        .into_iter()
        .filter_map(|row| match row {
            Row::Line(index) => {
                let line = rendered.get_mut(index)?.take()?;
                Some(if headers[index] {
                    header_bar(line, width)
                } else {
                    line
                })
            }
            // Eighth blocks, so each rule hugs the edge of the bar it borders
            // rather than floating mid-row.
            Row::RuleAbove => Some(rule('▁', width)),
            Row::RuleBelow => Some(rule('▔', width)),
        })
        .collect()
}

/// A file's header line as a bar across the panel.
fn header_bar(mut line: Line<'static>, width: usize) -> Line<'static> {
    let background = Style::default().bg(theme::DIFF_FILE_HEADER_BACKGROUND);
    line.style = line.style.patch(background);
    for span in &mut line.spans {
        span.style = span.style.patch(background);
    }
    let pad = width.saturating_sub(line.width());
    if pad > 0 {
        line.spans.push(Span::styled(" ".repeat(pad), background));
    }
    line
}

/// A line of `glyph` across the panel, drawn as characters rather than as an
/// underline: a terminal that does not colour underlines draws one in a
/// colour of its own.
fn rule(glyph: char, width: usize) -> Line<'static> {
    Line::from(Span::styled(
        glyph.to_string().repeat(width),
        Style::default().fg(theme::DIFF_FILE_RULE),
    ))
}

/// For each row `text` is drawn as with nothing hidden, whether hiding
/// removed lines hides it.
pub(crate) fn removed_rows(text: &str) -> Vec<bool> {
    diff_block_lines(text, None, false, "")
        .iter()
        .map(is_removed)
        .collect()
}

fn is_removed(line: &Line<'_>) -> bool {
    line.style.bg == Some(theme::DIFF_REMOVED_BACKGROUND)
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
    let names: Vec<_> = view
        .base_names
        .iter()
        .filter(|name| ![view.branch, view.base_branch].contains(&Some(name.as_str())))
        .collect();
    if !names.is_empty() {
        let muted = Style::default().fg(theme::MUTED_TEXT);
        comparison.push(Span::styled(" (", muted));
        for (position, name) in names.into_iter().enumerate() {
            if position > 0 {
                comparison.push(Span::styled(", ", muted));
            }
            comparison.push(Span::styled(name.clone(), Style::default().fg(theme::TEXT)));
        }
        comparison.push(Span::styled(")", muted));
    }
    if view.branch_point {
        comparison.push(Span::styled(
            "  (where the branch started)",
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    let mut lines = vec![
        Line::from(comparison),
        Line::from(vec![
            Span::styled(
                view.worktree.display().to_string(),
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
        "f: combined diff · /: search · j/k: files · J/K: 5 files · ↑/↓: scroll 10 · PgUp/PgDn: half-screen"
    } else {
        "f: per-file diffs · j/k: scroll · ↑/↓: scroll 10 · PgUp/PgDn: half-screen"
    };
    lines.push(muted(navigation));
    let removed = if view.hide_removed {
        "r: show removed lines · removed lines hidden"
    } else {
        "r: hide removed lines"
    };
    // On the same row, so the header is as tall in either view.
    let directories = match (view.per_file, view.compact_dirs) {
        (false, _) => "",
        (true, true) => " · c: unfold directories",
        (true, false) => " · c: fold directories",
    };
    let tint = match view.tint {
        Tint::Full => "t: halve unchanged tint",
        Tint::Half => "t: full unchanged tint",
    };
    lines.push(muted(&format!(
        "v: edit file · {removed} · {tint}{directories}"
    )));
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
        let lines = patch_lines(DIFF, true, Tint::Full, 80);
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
        assert!(patch_lines(DIFF, false, Tint::Full, 80)
            .iter()
            .any(|line| line.style.bg == Some(theme::DIFF_REMOVED_BACKGROUND)));
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
        search: SearchView<'_>,
    ) -> ((String, u16), u16) {
        let (width, height) = (80, 18);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut measured = CheckoutDiffFeedback::default();
        terminal
            .draw(|frame| {
                measured = render(
                    frame,
                    &CheckoutDiffView {
                        block: Block::bordered(),
                        worktree: Path::new("/state/worktrees/styra-7"),
                        branch: Some("styra/rename"),
                        base_branch,
                        base_commit: "4bf5c35d0123456789abcdef",
                        base_names: &[],
                        branch_point: true,
                        diff,
                        requested_scroll: scroll,
                        per_file,
                        hide_removed: false,
                        compact_dirs: true,
                        tint: Tint::Full,
                        selected_file,
                        file_list_offset: 0,
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
        ((output, measured.scroll.limit), measured.scroll.viewport)
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
            SearchView {
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
            SearchView {
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
    fn per_file_view_keeps_files_on_the_left_and_only_the_selected_patch_on_the_right() {
        let text =
            format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old second\n+new second\n");
        let ((output, _), viewport) = file_screen(Ok(&text), Some("main"), 0, true, 1);
        let rows = output.lines().collect::<Vec<_>>();
        // The panel border, marker, widest row ("  a.rs [M]", 10),
        // padding and tree border put the patch at column 16.
        let tree = |row: &&str| row.chars().take(16).collect::<String>();
        let patch = |row: &&str| row.chars().skip(16).collect::<String>();
        for name in ["▾ src/", "  a.rs [M]", "› b.txt"] {
            assert!(rows.iter().any(|row| tree(row).contains(name)), "{output}");
        }
        assert!(
            rows.iter()
                .any(|row| tree(row).contains("› b.txt") && tree(row).ends_with("  │")),
            "{output}"
        );
        assert!(
            rows.iter().any(|row| patch(row).contains("new second")),
            "{output}"
        );
        assert!(!output.contains("fn c()"), "{output}");
        assert!(output.contains("2 files changed"), "{output}");
        assert_eq!(viewport, 9);
    }

    #[test]
    fn file_list_shows_markers_and_explains_the_selected_change() {
        let text = "diff --git a/a b/a\nnew file mode 100644\n\
            diff --git a/b b/b\ndeleted file mode 100644\n\
            diff --git a/old b/c\nsimilarity index 100%\nrename from old\nrename to c\n\
            diff --git a/old2 b/d\nsimilarity index 90%\nrename from old2\nrename to d\n@@ -1 +1 @@\n-old\n+new\n";
        let ((output, _), _) = file_screen(Ok(text), Some("main"), 0, true, 0);
        for label in ["a [A]", "b [D]", "c [R]", "d [R+]"] {
            assert!(output.contains(label), "{output}");
        }
        assert!(output.contains("a · added"), "{output}");
        let ((output, _), _) = file_screen(Ok(text), Some("main"), u16::MAX, true, 3);
        assert!(
            output.contains("d · moved with edits from old2"),
            "{output}"
        );
        assert!(!output.contains("a · added"), "{output}");
        let ((output, _), _) = filtered_screen(
            Ok(text),
            Some("main"),
            0,
            true,
            0,
            SearchView {
                query: Some("c"),
                ..Default::default()
            },
        );
        assert!(output.contains("c · moved from old"), "{output}");
        let files = filtered_file_diffs(text, Some("c"));
        let rows = file_tree_rows(&files, true);
        assert_eq!(rows[0].0, Some(2));
        assert_eq!(rows[0].1.to_string(), "c [R]");
        assert_eq!(rows[0].1.spans[1].style.fg, Some(theme::DIRECTORY_NAME));
    }

    /// The tree fits its longest row, with a little room after it, but a
    /// deep path cannot take more than half the body from the patch.
    #[test]
    fn the_file_tree_is_as_wide_as_its_longest_row() {
        let rows = |paths: &[&str]| {
            let text = paths
                .iter()
                .map(|path| format!("diff --git a/{path} b/{path}\n"))
                .collect::<String>();
            let files = filtered_file_diffs(&text, None);
            file_tree_rows(&files, true)
                .into_iter()
                .map(|(index, line)| (index, Line::from(line.to_string())))
                .collect::<Vec<_>>()
        };
        // "  main.rs [M]": 13, + marker 2 + padding 2 + border 1.
        assert_eq!(file_tree_width(&rows(&["src/main.rs", "a.rs"]), 120), 18);
        let deep = ["a/very/deep/directory/structure/with/a_long_file_name.rs"];
        assert_eq!(file_tree_width(&rows(&deep), 60), 30);
        assert_eq!(file_tree_width(&[], 60), 5);
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
    fn the_selected_file_sits_on_a_soft_band_and_keeps_its_markers_color() {
        let text = "diff --git a/a b/a\nnew file mode 100644\n\
            diff --git a/b b/b\ndeleted file mode 100644\n";
        let mut terminal = Terminal::new(TestBackend::new(40, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        per_file: true,
                        diff: Ok(text),
                        selected_file: 1,
                        ..CheckoutDiffView::unread(Block::bordered())
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = |name: &str| {
            (0..12)
                .find(|&y| {
                    (0..40)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .contains(name)
                })
                .unwrap()
        };
        let (selected, other) = (row("b [D]"), row("a [A]"));
        // The marker, the name, and the change marker's bracket.
        for x in [1, 3, 5] {
            assert_eq!(buffer[(x, selected)].bg, theme::SELECTION_BACKGROUND);
            assert_ne!(buffer[(x, other)].bg, theme::SELECTION_BACKGROUND);
        }
        assert_eq!(buffer[(3, selected)].fg, theme::SELECTED_FILE_NAME);
        assert_ne!(buffer[(3, other)].fg, theme::SELECTED_FILE_NAME);
        assert_eq!(buffer[(6, selected)].fg, theme::ERROR);
    }

    #[test]
    fn file_tree_scrolls_with_five_rows_of_margin_in_both_directions() {
        let text = (0..60)
            .map(|n| format!("diff --git a/src/file{n:02} b/src/file{n:02}\n"))
            .collect::<String>();
        let mut terminal = Terminal::new(TestBackend::new(80, 28)).unwrap();
        let mut offset = 0;
        // The directory takes the first row; the file's row is its index + 1.
        for (selected_file, expected_offset) in [
            (0, 0),
            (13, 0),
            (14, 1),
            (15, 2),
            (30, 17),
            (22, 17),
            (21, 17),
            (20, 16),
            (0, 0),
            (59, 41),
        ] {
            terminal
                .draw(|frame| {
                    let measured = render(
                        frame,
                        &CheckoutDiffView {
                            block: Block::bordered(),
                            worktree: Path::new("/w"),
                            branch: None,
                            base_branch: None,
                            base_commit: "4bf5c35d",
                            base_names: &[],
                            branch_point: true,
                            diff: Ok(&text),
                            requested_scroll: 0,
                            per_file: true,
                            hide_removed: false,
                            compact_dirs: true,
                            tint: Tint::Full,
                            selected_file,
                            file_list_offset: offset,
                            search: Default::default(),
                        },
                        frame.area(),
                    );
                    assert_eq!(measured.scroll.viewport, 19);
                    offset = measured.file_list_offset;
                })
                .unwrap();
            assert_eq!(offset, expected_offset, "selected file {selected_file}");
            let selection_row = (0..28)
                .find(|&y| terminal.backend().buffer()[(1, y)].symbol() == "›")
                .expect("selection remains visible");
            if (14..=30).contains(&selected_file) {
                // The body spans rows 7 through 26, inclusive.
                assert!(selection_row >= 7 + 5);
                assert!(selection_row <= 26 - 5);
            }
        }
    }

    #[test]
    fn tree_groups_shared_directories_and_keeps_original_patch_indices_when_filtered() {
        let text = [
            "src/a.rs",
            "src/a/z.rs",
            "README.md",
            "src/a/b.rs",
            "src/z.rs",
        ]
        .iter()
        .map(|path| format!("diff --git a/{path} b/{path}\n"))
        .collect::<String>();
        let files = filtered_file_diffs(&text, None);
        let rows = file_tree_rows(&files, false);
        assert_eq!(
            rows.len(),
            file_tree_rows(&files, true).len(),
            "nothing to fold"
        );
        let displayed: Vec<_> = rows
            .iter()
            .map(|(index, line)| (*index, line.to_string()))
            .collect();
        assert_eq!(
            displayed,
            vec![
                (Some(2), "README.md [M]".into()),
                (None, "▾ src/".into()),
                (None, "  ▾ a/".into()),
                (Some(3), "    b.rs [M]".into()),
                (Some(1), "    z.rs [M]".into()),
                (Some(0), "  a.rs [M]".into()),
                (Some(4), "  z.rs [M]".into()),
            ]
        );
        let files = filtered_file_diffs(&text, Some("src/a/"));
        assert_eq!(
            files.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            [3, 1]
        );
        assert_eq!(file_tree_rows(&files, false).len(), 4);
        assert_eq!(file_tree_rows(&files, true).len(), 3, "src/ holds only a/");
    }

    /// Directories with no files of their own and one subdirectory fold onto
    /// a single row; one with several subdirectories keeps its own. Folded or
    /// not, directories are told from files by colour.
    #[test]
    fn directories_without_files_fold_into_one_row() {
        let text = [
            "styra/diff/src/lib.rs",
            "styra/diff/src/view.rs",
            "styra/ui/src/scroll.rs",
            "styra/ui/src/search.rs",
            "Cargo.toml",
        ]
        .iter()
        .map(|path| format!("diff --git a/{path} b/{path}\n"))
        .collect::<String>();
        let files = filtered_file_diffs(&text, None);
        let shown = |compact| {
            file_tree_rows(&files, compact)
                .iter()
                .map(|(_, line)| line.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            shown(true),
            [
                "Cargo.toml [M]",
                "▾ styra/",
                "  ▾ diff/src/",
                "    lib.rs [M]",
                "    view.rs [M]",
                "  ▾ ui/src/",
                "    scroll.rs [M]",
                "    search.rs [M]",
            ]
        );
        assert_eq!(
            shown(false),
            [
                "Cargo.toml [M]",
                "▾ styra/",
                "  ▾ diff/",
                "    ▾ src/",
                "      lib.rs [M]",
                "      view.rs [M]",
                "  ▾ ui/",
                "    ▾ src/",
                "      scroll.rs [M]",
                "      search.rs [M]",
            ]
        );
        for (index, line) in file_tree_rows(&files, true) {
            let directory = line.spans[0].style.fg == Some(theme::DIRECTORY_NAME);
            assert_eq!(directory, index.is_none(), "{line}");
        }
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
        let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        block: Block::bordered(),
                        worktree: Path::new("/w"),
                        branch: None,
                        base_branch: None,
                        base_commit: "4bf5c35d",
                        base_names: &[],
                        branch_point: true,
                        diff: Ok(DIFF),
                        requested_scroll: 0,
                        per_file: false,
                        hide_removed: false,
                        compact_dirs: true,
                        tint: Tint::Full,
                        selected_file: 0,
                        file_list_offset: 0,
                        search: Default::default(),
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = (0..18u16)
            .find(|&y| {
                (0..80u16)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains("fn c()")
            })
            .unwrap();
        assert_eq!(buffer[(70, row)].bg, theme::DIFF_ADDED_BACKGROUND);
    }

    /// Each file's header is a bar between two rules drawn as rows of their
    /// own: one along its bottom, and — for every file after the first — one
    /// along its top. Hidden removed lines take no rule with them.
    #[test]
    fn file_headers_are_bars_between_rules() {
        let text = format!(
            "{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n+kept\n-gone\n\
             diff --git a/c.txt b/c.txt\n@@ -1 +1 @@\n-x\n+y\n"
        );
        // The rows beneath the view's own header: a bar in brackets, a rule
        // as its glyph alone, and any other row as its text.
        let body = |hide_removed: bool| {
            let height = 40;
            let mut terminal = Terminal::new(TestBackend::new(80, height)).unwrap();
            let frame = terminal
                .draw(|frame| {
                    render(
                        frame,
                        &CheckoutDiffView {
                            diff: Ok(&text),
                            hide_removed,
                            ..CheckoutDiffView::unread(Block::bordered())
                        },
                        frame.area(),
                    );
                })
                .unwrap();
            let buffer = frame.buffer.clone();
            let rows: Vec<(String, bool, bool)> = (0..height)
                .map(|y| {
                    let cells: Vec<_> = (1..79u16).map(|x| &buffer[(x, y)]).collect();
                    let text: String = cells.iter().map(|cell| cell.symbol()).collect();
                    let bar = cells
                        .iter()
                        .all(|cell| cell.bg == theme::DIFF_FILE_HEADER_BACKGROUND);
                    let rule = cells.iter().all(|cell| cell.fg == theme::DIFF_FILE_RULE);
                    (text, bar, rule)
                })
                .collect();
            // Inside the panel's own border, past the rule under the header.
            let inside = &rows[1..rows.len() - 1];
            let start = inside
                .iter()
                .position(|(text, ..)| text.starts_with("──"))
                .unwrap();
            inside[start + 1..]
                .iter()
                .map(|(text, bar, rule)| match (bar, rule) {
                    (true, _) => format!("[{}]", text.trim()),
                    (_, true) => text.chars().next().unwrap().to_string(),
                    _ => text.trim().to_owned(),
                })
                .filter(|row| !row.is_empty())
                .collect::<Vec<_>>()
        };
        assert!(
            body(false).iter().all(|row| {
                row.chars().all(|glyph| glyph != '▁' && glyph != '▔') || row.chars().count() == 1
            }),
            "rules run edge to edge"
        );
        assert_eq!(
            body(false),
            [
                "[│ diff --git a/src/a.rs b/src/a.rs]",
                "▔",
                "│ index 1111111..2222222 100644",
                "│ --- a/src/a.rs",
                "│ +++ b/src/a.rs",
                "│ @@ -1,2 +1,3 @@",
                "│ 1  fn a() {}",
                "│ 2 -fn b() {}",
                "│ 2 +fn c() {}",
                "│ 3 +fn d() {}",
                "▁",
                "[│ diff --git a/b.txt b/b.txt]",
                "▔",
                "│ @@ -1 +1 @@",
                "│ 1 +kept",
                "│ 1 -gone",
                "▁",
                "[│ diff --git a/c.txt b/c.txt]",
                "▔",
                "│ @@ -1 +1 @@",
                "│ 1 -x",
                "│ 1 +y",
            ]
        );
        let hidden = body(true);
        assert_eq!(hidden.len(), body(false).len() - 3, "{hidden:?}");
        assert_eq!(
            hidden[13..16],
            ["│ 1 +kept", "▁", "[│ diff --git a/c.txt b/c.txt]"]
        );
    }

    /// `t` switches the unchanged rows between faded as everywhere else and
    /// half as faded; the bands stay throughout, and hiding removed lines
    /// does not depend on them.
    #[test]
    fn the_unchanged_rows_tint_switches_between_full_and_half() {
        let draw = |tint: Tint, hide_removed: bool| {
            let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
            let frame = terminal
                .draw(|frame| {
                    render(
                        frame,
                        &CheckoutDiffView {
                            diff: Ok(DIFF),
                            hide_removed,
                            tint,
                            ..CheckoutDiffView::unread(Block::bordered())
                        },
                        frame.area(),
                    );
                })
                .unwrap();
            let buffer = frame.buffer.clone();
            let rows: Vec<String> = (0..18u16)
                .map(|y| (0..80u16).map(|x| buffer[(x, y)].symbol()).collect())
                .collect();
            // The `f` opening a row's code.
            let start = |code: &str| {
                let y = rows.iter().position(|row| row.contains(code))?;
                let x = rows[y][..rows[y].find(code)?].chars().count();
                Some(&buffer[(x as u16, y as u16)])
            };
            let added = rows.iter().position(|row| row.contains("fn c()")).unwrap();
            (
                start("fn a()").unwrap().fg,
                start("fn c()").unwrap().fg,
                buffer[(70, added as u16)].bg,
                start("fn b()").is_some(),
            )
        };
        let (full, added, band, removed) = draw(Tint::Full, false);
        let (half, _, half_band, _) = draw(Tint::Half, false);
        assert_ne!(full, added);
        assert!(
            ![full, added].contains(&half),
            "{full:?} {half:?} {added:?}"
        );
        for band in [band, half_band] {
            assert_eq!(band, theme::DIFF_ADDED_BACKGROUND);
        }
        assert!(removed);
        assert!(!draw(Tint::Half, true).3);
        assert_eq!(Tint::Full.next().next(), Tint::Full);
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

    /// A base that is just a commit to compare with is not called where the
    /// branch started.
    #[test]
    fn a_base_that_is_no_branch_point_is_not_called_one() {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        let frame = terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        branch: Some("main"),
                        base_branch: Some("HEAD"),
                        base_commit: "4bf5c35d0123456789abcdef",
                        base_names: &[],
                        diff: Ok(""),
                        ..CheckoutDiffView::unread(Block::bordered())
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let output = frame
            .buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            output.contains("main  against  HEAD at 4bf5c35d01"),
            "{output}"
        );
        assert!(!output.contains("where the branch started"), "{output}");
        assert!(output.contains("no changes"), "{output}");
        assert!(!output.contains("since the branch was made"), "{output}");
    }

    /// The branches and tags at the base commit follow it, as `git log`
    /// decorates a commit — less the branches the header already names.
    #[test]
    fn the_names_at_the_base_commit_follow_it() {
        let names = [
            "main".to_owned(),
            "styra/rename".into(),
            "tag: v1.2".into(),
            "origin/main".into(),
        ];
        let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
        let frame = terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        branch: Some("styra/rename"),
                        base_branch: Some("main"),
                        base_commit: "4bf5c35d0123456789abcdef",
                        base_names: &names,
                        branch_point: true,
                        diff: Ok(DIFF),
                        ..CheckoutDiffView::unread(Block::bordered())
                    },
                    frame.area(),
                );
            })
            .unwrap();
        let output = frame
            .buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(
            output.contains(
                "against  main at 4bf5c35d01 (tag: v1.2, origin/main)  (where the branch started)"
            ),
            "{output}"
        );
    }
}
