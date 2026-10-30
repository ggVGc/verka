//! A working tree diffed against a commit, drawn into whatever area the host
//! gives it: a panel inside Styra, or the whole terminal for `styra-diff`.
//!
//! The diff alone does not say what it is a diff *of*, so a header above it
//! names both sides — the branch being worked on and the branch and commit it
//! started from — and the worktree they are read in. The header stays put
//! while the diff below it scrolls.

use crate::{
    layers::Layers,
    report::{ReportPage, ReportState},
};
#[cfg(feature = "renderer")]
use ratatui::{text::Line, widgets::Block};
use std::path::Path;
#[cfg(feature = "renderer")]
use styra_ui::diff::CONTEXT_FADE;
use styra_ui::{preview::PreviewFeedback, search::SearchView};
pub struct CheckoutDiffView<'a> {
    pub commits: Option<&'a crate::state::Commits>,
    /// Prompts attached to files and lines in this session.
    pub prompts: Vec<crate::Prompt>,
    /// The frame drawn around the view: Styra's panel chrome, or a plain
    /// border when the view runs on its own.
    #[cfg(feature = "renderer")]
    pub block: Block<'a>,
    pub chrome: Option<styra_ui::chrome::PanelChrome>,
    pub intro_message: Option<styra_ui::preview::MessageView<'a>>,
    pub conversation_data: Option<styra_ui::picker::ConversationView<'a>>,
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
    /// them, or whose tip trees exactly match a snapshot baseline.
    pub base_names: &'a [String],
    /// Whether that commit is where the checked-out branch started, which the
    /// header says, rather than just a commit to compare with.
    pub branch_point: bool,
    /// What a diff the host supplied is of, said in place of the branches
    /// and commit a checkout's diff compares; `None` for a checkout's diff.
    /// [`Self::worktree`] is then where its files are, or empty.
    pub label: Option<&'a str>,
    /// `git diff`'s output, or why there is none.
    pub diff: Result<&'a str, &'a str>,
    /// A selected turn patch whose surviving changes should stand out.
    pub highlight_diff: Option<&'a str>,
    pub requested_scroll: u16,
    /// Scroll position of the shortcut legend while it is open.
    pub help: Option<u16>,
    /// Additional shortcuts provided by the host, shown first in the legend.
    pub host_shortcuts: &'a [(&'a str, &'a str)],
    pub per_file: bool,
    pub side_by_side: bool,
    pub rendered_side: bool,
    pub horizontal_scroll: u16,
    /// Show the selected patch thumbnail beneath the file tree in fullscreen.
    pub thumbnail: bool,
    /// Whether the thumbnail has been put away, where it would be shown.
    pub thumbnail_hidden: bool,
    pub hide_removed: bool,
    /// Whether the file tree folds directories with no files of their own
    /// and a single subdirectory onto one row.
    pub compact_dirs: bool,
    /// Whether the per-file view lists its files in a strip under the patch,
    /// as in a side panel too narrow to give them a column of their own.
    pub files_below: bool,
    pub tint: Tint,
    pub word_highlights: bool,
    /// While changes are tinted by the commits that made them, those
    /// commits, or why they could not be read.
    pub layers: Option<Result<&'a Layers, &'a str>>,
    /// Commits pinned by the involved-files filter.
    pub involved_layers: Option<&'a [u16]>,
    pub selected_file: usize,
    /// First visible row in the file tree, retained between frames.
    pub file_list_offset: usize,
    pub search: SearchView<'a>,
    /// The edit report the per-file view describes the selected file from.
    /// `None` where no report can be had, as in `styra-diff` on its own,
    /// which leaves its keys out of the shortcut legend too.
    pub report: Option<&'a ReportState>,
    /// The page of every file's description, drawn over the view while it
    /// is open.
    pub report_page: Option<ReportPage>,
    /// Lines the host shows in place of the header, such as the latest
    /// messages of the conversation the diff belongs to. `None` keeps the
    /// header.
    #[cfg(feature = "renderer")]
    pub conversation: Option<Vec<Line<'a>>>,
    /// Lines the host shows above the header, such as the message that
    /// started the turn a turn diff is of. Wrapped to the panel, and given
    /// no more than half of it.
    #[cfg(feature = "renderer")]
    pub intro: Option<Vec<Line<'a>>>,
    /// The line the line cursor is on, among the shown patch's lines; `None`
    /// unless lines are being selected.
    pub line_cursor: Option<usize>,
    /// The text to find in line selection, marked wherever it is drawn.
    pub find: SearchView<'a>,
}

impl<'a> CheckoutDiffView<'a> {
    /// The view before any diff has been read into it, saying so.
    #[cfg(feature = "renderer")]
    pub fn unread(block: Block<'a>) -> Self {
        let mut view = Self::empty();
        view.block = block;
        view
    }

    /// A host panel described without terminal widgets or styled text.
    pub fn from_chrome(chrome: styra_ui::chrome::PanelChrome) -> Self {
        let mut view = Self::empty();
        view.chrome = Some(chrome);
        view
    }

    pub fn empty() -> Self {
        Self {
            #[cfg(feature = "renderer")]
            block: ratatui::widgets::Block::bordered(),
            chrome: None,
            intro_message: None,
            conversation_data: None,
            commits: None,
            worktree: Path::new(""),
            branch: None,
            base_branch: None,
            base_commit: "",
            base_names: &[],
            branch_point: false,
            label: None,
            diff: Err("no diff read yet"),
            highlight_diff: None,
            requested_scroll: 0,
            help: None,
            host_shortcuts: &[],
            per_file: false,
            side_by_side: false,
            rendered_side: false,
            horizontal_scroll: 0,
            thumbnail: false,
            thumbnail_hidden: false,
            hide_removed: false,
            compact_dirs: true,
            files_below: false,
            tint: Tint::Full,
            word_highlights: true,
            layers: None,
            involved_layers: None,
            selected_file: 0,
            file_list_offset: 0,
            search: SearchView::default(),
            report: None,
            report_page: None,
            #[cfg(feature = "renderer")]
            conversation: None,
            #[cfg(feature = "renderer")]
            intro: None,
            line_cursor: None,
            find: SearchView::default(),
            prompts: Vec::new(),
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
    #[cfg(feature = "renderer")]
    fn fade(self) -> u16 {
        match self {
            Self::Full => CONTEXT_FADE,
            Self::Half => CONTEXT_FADE / 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CheckoutDiffFeedback {
    pub side_by_side: bool,
    pub horizontal_scroll: u16,
    pub scroll: PreviewFeedback,
    pub file_list_offset: usize,
    /// Where the report page's selection and scroll were held to, while it
    /// is open.
    pub report_page: Option<ReportPage>,
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

/// The rows `text` is drawn as: Git index and path markers are omitted, as
/// are removed lines when hidden, and each file's header is set between two
/// rules — the one above only when something is drawn above it.
pub(crate) fn rows(text: &str, hide_removed: bool, per_file: bool) -> Vec<Row> {
    let removed = if hide_removed {
        removed_rows(text)
    } else {
        Vec::new()
    };
    file_view_rows(text, plan(text, &removed), per_file)
}

/// Keep the full patch for highlighting and source positions, but omit its
/// repeated Git header when the selected file already has a title.
fn file_view_rows(text: &str, rows: Vec<Row>, per_file: bool) -> Vec<Row> {
    if !per_file {
        return rows;
    }
    let lines: Vec<_> = text.lines().collect();
    let first_hunk = lines.iter().position(|line| line.starts_with("@@"));
    rows.into_iter()
        .filter(|row| match *row {
            Row::Line(index) => first_hunk.map_or_else(
                || {
                    let line = lines[index];
                    !["diff --git ", "index ", "--- ", "+++ "]
                        .iter()
                        .any(|prefix| line.starts_with(prefix))
                },
                |hunk| index >= hunk,
            ),
            Row::RuleAbove | Row::RuleBelow => false,
        })
        .collect()
}

/// [`rows`], given which lines are `removed` and to be left out.
fn plan(text: &str, removed: &[bool]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut in_hunk = false;
    for (index, line) in text.lines().enumerate() {
        if is_file_header(line) {
            in_hunk = false;
        } else if is_hunk_header(line) {
            in_hunk = true;
        } else if !in_hunk
            && ["index ", "--- ", "+++ "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        {
            continue;
        }
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

fn is_hunk_header(line: &str) -> bool {
    line.starts_with("@@")
}

/// For each row `text` is drawn as with nothing hidden, whether hiding
/// removed lines hides it.
pub(crate) fn removed_rows(text: &str) -> Vec<bool> {
    text.lines()
        .map(|line| {
            line.starts_with('-')
                && !line.starts_with("--- a/")
                && !line.starts_with("--- /dev/null")
        })
        .collect()
}

/// The layers that touched any line of the block of changes `cursor` is in:
/// the run of added and removed lines around it. None when it is on no
/// change.
pub(crate) fn block_layers(layers: &Layers, text: &str, cursor: usize) -> Vec<u16> {
    let lines: Vec<&str> = text.lines().collect();
    // Within a hunk every line starting so is a change, or the note that
    // a side has no newline at its end; file markers come before any hunk
    // and are never marked.
    let changed = |line: usize| {
        lines
            .get(line)
            .is_some_and(|line| line.starts_with(['+', '-', '\\']))
    };
    if !lines
        .get(cursor)
        .is_some_and(|line| line.starts_with(['+', '-']))
    {
        return Vec::new();
    }
    let start = (0..cursor)
        .rev()
        .take_while(|&line| changed(line))
        .last()
        .unwrap_or(cursor);
    let end = (cursor..lines.len())
        .take_while(|&line| changed(line))
        .last()
        .unwrap_or(cursor);
    let marks = layers.marks(text);
    let mut touched: Vec<u16> = marks[start..=end.min(marks.len().saturating_sub(1))]
        .iter()
        .flatten()
        .copied()
        .collect();
    touched.sort_unstable();
    touched.dedup();
    touched
}

#[cfg(not(feature = "renderer"))]
impl styra_ui::application::Panel for CheckoutDiffView<'_> {}
#[cfg(feature = "renderer")]
pub use rendering::*;
#[cfg(feature = "renderer")]
#[path = "view/render.rs"]
mod rendering;
