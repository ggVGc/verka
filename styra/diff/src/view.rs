//! A working tree diffed against a commit, drawn into whatever area the host
//! gives it: a panel inside Styra, or the whole terminal for `styra-diff`.
//!
//! The diff alone does not say what it is a diff *of*, so a header above it
//! names both sides — the branch being worked on and the branch and commit it
//! started from — and the worktree they are read in. The header stays put
//! while the diff below it scrolls.

use crate::files::{filtered_file_diffs, FileChange, FileDiff};
use crate::layers::Layers;
use crate::report::{Described, ReportState};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::io::BufRead;
use std::path::Path;
use styra_ui::application::{Panel, PanelFeedback};
use styra_ui::diff::{diff_block_lines, diff_block_lines_fading, fill_changed_rows, CONTEXT_FADE};
use styra_ui::event_list::wrap_rendered;
use styra_ui::markdown::{markdown_block_lines_with_links, LinkDisplay};
use styra_ui::preview::PreviewFeedback;
use styra_ui::search::SearchView;
use styra_ui::theme;

pub struct CheckoutDiffView<'a> {
    /// Prompts attached to files and lines in this session.
    pub prompts: Vec<crate::Prompt>,
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
    /// Lines the host shows in place of the header, such as the latest
    /// messages of the conversation the diff belongs to. `None` keeps the
    /// header.
    pub conversation: Option<Vec<Line<'a>>>,
    /// Lines the host shows above the header, such as the message that
    /// started the turn a turn diff is of. Wrapped to the panel, and given
    /// no more than half of it.
    pub intro: Option<Vec<Line<'a>>>,
    /// The line the line cursor is on, among the shown patch's lines; `None`
    /// unless lines are being selected.
    pub line_cursor: Option<usize>,
    /// The text to find in line selection, marked wherever it is drawn.
    pub find: SearchView<'a>,
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
            conversation: None,
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
}

/// Styra draws the view as its main panel, which `styra-ui` cannot name.
impl Panel for CheckoutDiffView<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) -> PanelFeedback {
        let measured = render(frame, self, area);
        PanelFeedback {
            scroll: measured.scroll,
            list_offset: Some(measured.file_list_offset),
            diff_layout: Some((measured.side_by_side, measured.horizontal_scroll)),
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
                    rows.push((
                        Some(*index),
                        Line::from(vec![
                            Span::raw(format!("{indent}{name}")),
                            change_marker(*change),
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

/// The most file rows the strip under the patch shows when the files are
/// listed there; the list scrolls through the rest.
const FILE_LIST_MAX_HEIGHT: u16 = 10;

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
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if let Some(intro) = &view.intro {
        let paragraph = Paragraph::new(intro.clone()).wrap(Wrap { trim: false });
        // One more for the rule beneath it.
        let wanted = paragraph.line_count(inner.width.max(1)).saturating_add(1);
        let height = u16::try_from(wanted)
            .unwrap_or(u16::MAX)
            .min(inner.height / 2);
        let [top, rest] =
            Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).areas(inner);
        frame.render_widget(
            paragraph.block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(theme::INACTIVE)),
            ),
            top,
        );
        inner = rest;
    }
    let header = view
        .conversation
        .clone()
        .unwrap_or_else(|| header_lines(view));
    // The combined diff has no left pane to list the layers in: its top
    // bar shares them, on its left, and is as tall as they need.
    let top_legend = view.layers.filter(|_| !view.per_file);
    let legend_height = top_legend.map_or(0, |layers| legend_rows(layers, &[]).len());
    let [top, body] = Layout::vertical([
        // One more than the header for the rule beneath it.
        Constraint::Length(header.len().max(legend_height) as u16 + 1),
        Constraint::Min(0),
    ])
    .areas(inner);
    frame.render_widget(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(theme::INACTIVE)),
        top,
    );
    let top = Rect::new(top.x, top.y, top.width, top.height.saturating_sub(1));
    // Filled once what the line cursor is over is known.
    let (top, legend_area) = match top_legend {
        Some(layers) => {
            let width = legend_width(&legend_rows(layers, &[])).min(top.width / 2);
            let [legend, header] =
                Layout::horizontal([Constraint::Length(width), Constraint::Min(0)]).areas(top);
            frame.render_widget(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(theme::INACTIVE)),
                legend,
            );
            let inner = Rect::new(
                legend.x,
                legend.y,
                legend.width.saturating_sub(1),
                legend.height,
            );
            // A column's gap between the rule and the header.
            let header = Rect::new(
                header.x.saturating_add(1),
                header.y,
                header.width.saturating_sub(1),
                header.height,
            );
            (header, Some((layers, inner)))
        }
        None => (top, None),
    };
    frame.render_widget(Paragraph::new(header), top);
    let mut files = view
        .diff
        .ok()
        .map(|text| filtered_file_diffs(text, view.search.query))
        .unwrap_or_default();
    if view.per_file {
        if let (Some(touched), Some(Ok(layers))) = (view.involved_layers, view.layers) {
            files.retain(|(_, file)| {
                layers
                    .file_layers(file.patch)
                    .iter()
                    .any(|i| touched.contains(i))
            });
        }
    }
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
    // The commits that touched the block of changes under the line cursor.
    let touched = match (view.line_cursor, view.layers, shown) {
        (Some(cursor), Some(Ok(layers)), Ok(text)) => block_layers(layers, text, cursor),
        _ => Vec::new(),
    };
    let mut thumbnail_area = None;
    let mut file_list_offset = view.file_list_offset;
    let body = if view.per_file {
        let mut rows = file_tree_rows(&files, view.compact_dirs);
        for (index, line) in &mut rows {
            if index
                .and_then(|index| files.iter().find(|(at, _)| *at == index))
                .is_some_and(|(_, file)| view.prompts.iter().any(|p| p.path == file.path))
            {
                line.spans.push(prompt_marker());
            }
        }
        let stat = view.diff.ok().map(stat_line);
        let stat_width = stat.as_ref().map_or(0, |stat| {
            u16::try_from(stat.width())
                .unwrap_or(u16::MAX)
                .saturating_add(FILE_TREE_PADDING + 1)
        });
        // Outside line selection, highlight all changes in the focused file.
        let file_touched = match (view.line_cursor, view.layers, selected_file) {
            (None, Some(Ok(layers)), Some(file)) => layers.file_layers(file.patch),
            _ => touched.clone(),
        };
        let file_touched = view.involved_layers.unwrap_or(&file_touched);
        let legend = view.layers.map(|layers| {
            let mut rows = legend_rows(layers, file_touched);
            if let (Ok(layers), Ok(text)) = (layers, view.diff) {
                let files = filtered_file_diffs(text, view.search.query);
                let involved = files
                    .iter()
                    .filter(|(_, file)| {
                        layers
                            .file_layers(file.patch)
                            .iter()
                            .any(|i| file_touched.contains(i))
                    })
                    .count();
                let action = if view.involved_layers.is_some() {
                    "show all"
                } else {
                    "filter"
                };
                rows.insert(
                    0,
                    muted(&format!(
                        "{involved}/{} files involved · c: {action}",
                        files.len()
                    )),
                );
            }
            rows
        });
        let legend_wide = legend.as_deref().map_or(0, legend_width);
        // The files in a strip under the patch, no taller than they need,
        // nor than FILE_LIST_MAX_HEIGHT, nor than half the body. The
        // thumbnail has no room there.
        let thumbnail = view.thumbnail && !view.thumbnail_hidden && !view.files_below;
        let (tree, right, legend) = if view.files_below {
            let listed = rows.len() + usize::from(stat.is_some());
            let wanted = listed.max(legend.as_ref().map_or(0, Vec::len));
            // One more for the rule above the strip.
            let height = u16::try_from(wanted)
                .unwrap_or(u16::MAX)
                .min(FILE_LIST_MAX_HEIGHT)
                .saturating_add(1)
                .min(body.height / 2);
            let [patch, strip] =
                Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(body);
            frame.render_widget(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme::INACTIVE)),
                strip,
            );
            let strip = Rect::new(
                strip.x,
                strip.y.saturating_add(1),
                strip.width,
                strip.height.saturating_sub(1),
            );
            // The layers to the files' right, behind a rule of their own.
            let tree = match legend {
                Some(legend) => {
                    let width = legend_wide.saturating_add(1).min(strip.width / 2);
                    let [files, beside] =
                        Layout::horizontal([Constraint::Min(0), Constraint::Length(width)])
                            .areas(strip);
                    frame.render_widget(
                        Block::default()
                            .borders(Borders::LEFT)
                            .border_style(Style::default().fg(theme::INACTIVE)),
                        beside,
                    );
                    let column = Rect::new(
                        beside.x.saturating_add(1),
                        beside.y,
                        beside.width.saturating_sub(1),
                        beside.height,
                    );
                    render_legend(frame, legend, column);
                    files
                }
                None => strip,
            };
            (tree, patch, None)
        } else {
            let tree_width = if thumbnail {
                file_tree_width(&rows, body.width).max(24.min(body.width / 2))
            } else {
                file_tree_width(&rows, body.width)
            }
            .max(stat_width.min(body.width / 2))
            .max(legend_wide.min(body.width / 2));
            let [left, right] =
                Layout::horizontal([Constraint::Length(tree_width), Constraint::Min(0)])
                    .areas(body);
            frame.render_widget(
                Block::default()
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(theme::INACTIVE)),
                left,
            );
            (
                Rect::new(left.x, left.y, left.width.saturating_sub(1), left.height),
                right,
                legend,
            )
        };
        let thumbnail_height =
            if thumbnail && selected_file.is_some() && tree.width >= 11 && tree.height >= 19 {
                17
            } else {
                0
            };
        let left = tree;
        let tree = Rect::new(tree.x, tree.y, tree.width, tree.height - thumbnail_height);
        // The layers take what they need beneath the files, with a rule above
        // them, but never more than half of what the two share.
        let tree = match legend {
            Some(legend) => {
                let height = u16::try_from(legend.len())
                    .unwrap_or(u16::MAX)
                    .saturating_add(1)
                    .min(tree.height / 2);
                let [files, below] =
                    Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(tree);
                frame.render_widget(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(theme::INACTIVE)),
                    below,
                );
                let rows = Rect::new(
                    below.x,
                    below.y.saturating_add(1),
                    below.width,
                    below.height.saturating_sub(1),
                );
                render_legend(frame, legend, rows);
                files
            }
            None => tree,
        };
        // How much the diff changes, above the files it changes.
        let tree = match stat {
            Some(stat) => {
                let [stat_row, list] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(tree);
                frame.render_widget(Paragraph::new(stat), stat_row);
                list
            }
            None => tree,
        };
        if thumbnail_height > 0 {
            thumbnail_area = Some(Rect::new(
                left.x,
                left.bottom() - thumbnail_height,
                tree.width.min(30),
                thumbnail_height,
            ));
        }
        let selected = rows
            .iter()
            .position(|(index, _)| *index == Some(view.selected_file));
        // The files those commits changed stand out, and the others are
        // muted, as the commits are in the legend.
        if let (Some(Ok(layers)), false) = (view.layers, file_touched.is_empty()) {
            for (index, line) in &mut rows {
                let Some(file) = index.and_then(|index| {
                    files
                        .iter()
                        .find(|(at, _)| *at == index)
                        .map(|(_, file)| file)
                }) else {
                    continue;
                };
                let changed = layers
                    .file_layers(file.patch)
                    .iter()
                    .any(|layer| file_touched.contains(layer));
                if let Some(name) = line.spans.first_mut() {
                    name.style = if changed {
                        name.style.fg(theme::TEXT).add_modifier(Modifier::BOLD)
                    } else {
                        name.style.fg(theme::MUTED_TEXT)
                    };
                }
            }
        }
        // Only the name: its change marker keeps its own color.
        if let Some(name) = selected.and_then(|row| rows[row].1.spans.first_mut()) {
            name.style = name.style.fg(theme::SELECTED_FILE_NAME);
        }
        let items = rows.into_iter().map(|(_, line)| ListItem::new(line));
        let list = List::new(items)
            .scroll_padding(5)
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
        frame.render_stateful_widget(list, tree, &mut state);
        file_list_offset = state.offset();
        right
    } else {
        body
    };
    let body = if let Some(file) = selected_file {
        let [info, patch] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        let mut label = file_info(file);
        if view.prompts.iter().any(|p| p.path == file.path) {
            label.spans.push(prompt_marker());
        }
        frame.render_widget(Paragraph::new(label), info);
        match view.report.filter(|report| report.shown()) {
            Some(report) => render_description(frame, report, file, patch),
            None => patch,
        }
    } else {
        body
    };
    let side_by_side = view.side_by_side && !view.hide_removed && body.width >= 81;
    let mut body = body;
    if view.side_by_side {
        let [label, rest] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        let label_text = if view.hide_removed {
            "unified · removed lines hidden".to_owned()
        } else if !side_by_side {
            "unified · side-by-side needs more width (81 columns)".to_owned()
        } else if view.label.is_some() {
            // As `render_split` draws them: the newer side on the left.
            let half = usize::from(body.width / 2);
            format!("{:<half$}│ Before", "After")
        } else {
            let half = usize::from(body.width / 2);
            let base = format!(
                "Base · {}",
                view.base_commit
                    .get(..SHORT_COMMIT)
                    .unwrap_or(view.base_commit)
            );
            format!("{:<half$}│ {base}", "Working tree")
        };
        frame.render_widget(Paragraph::new(muted(&label_text)), label);
        body = rest;
    }
    if let (Some(cursor), Some(Ok(layers)), Ok(text)) = (view.line_cursor, view.layers, shown) {
        let marks = layers.marks(text);
        let mark = marks.get(cursor).map(Vec::as_slice).unwrap_or_default();
        let rows = line_layers(layers, mark);
        // Only as tall as the cursor's line needs, and never more than half.
        let height = u16::try_from(rows.len())
            .unwrap_or(u16::MAX)
            .saturating_add(1)
            .min(body.height / 2);
        let [rest, info] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(body);
        frame.render_widget(
            Paragraph::new(rows).block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(theme::INACTIVE)),
            ),
            info,
        );
        body = rest;
    }
    let source = match (view.line_cursor, shown) {
        (Some(cursor), Ok(text)) => crate::files::position(text, cursor),
        _ => selected_file.map(|file| (file.path, None)),
    };
    let prompts: Vec<_> = view
        .prompts
        .iter()
        .filter(|p| source.is_some_and(|source| p.matches(source)))
        .collect();
    if !prompts.is_empty() {
        let text = prompts
            .iter()
            .rev()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
        let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
        let height = u16::try_from(paragraph.line_count(body.width.saturating_sub(2).max(1)))
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .min(body.height / 2);
        let [rest, info] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(height)]).areas(body);
        frame.render_widget(
            paragraph.block(
                Block::bordered()
                    .title(" Prompt · Enter: interaction log (latest) ")
                    .border_style(Style::default().fg(theme::ACCENT)),
            ),
            info,
        );
        body = rest;
    }
    // The text to find, along the bottom with how many lines it is on and
    // which of them the line cursor is on, as Vim counts its matches.
    let find = view.find.query.filter(|term| !term.is_empty());
    if let (Some(query), Some(cursor)) = (view.find.query, view.line_cursor) {
        let [rest, prompt] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(body);
        let found = match (find, shown) {
            (Some(term), Ok(text)) => {
                crate::find::match_rows(text, term, view.hide_removed, side_by_side, view.per_file)
            }
            _ => Vec::new(),
        };
        let mut spans = vec![Span::styled(
            format!(" /{query}{}", if view.find.typing { "▌" } else { "" }),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )];
        let at = match shown {
            Ok(text) => {
                crate::split::row_of(text, view.hide_removed, side_by_side, view.per_file, cursor)
            }
            Err(_) => usize::MAX,
        };
        let status = match found.iter().position(|&(row, _)| row == at) {
            _ if find.is_none() => String::new(),
            _ if found.is_empty() => "no match".to_owned(),
            Some(at) => format!("[{}/{}]", at + 1, found.len()),
            None => format!("{} lines", found.len()),
        };
        let keys = if view.find.typing {
            "Enter: keep · Esc: cancel"
        } else {
            "n / N: next / previous · Esc: clear"
        };
        spans.push(Span::styled(
            format!(
                "  {status}{}{keys}",
                if status.is_empty() { "" } else { "  · " }
            ),
            Style::default().fg(theme::MUTED_TEXT),
        ));
        frame.render_widget(Paragraph::new(Line::from(spans)), prompt);
        body = rest;
    }
    let find = find.filter(|_| view.line_cursor.is_some());
    let width = usize::from(body.width);
    let requested = shown
        .ok()
        .map(|text| {
            crate::split::remap(
                text,
                view.hide_removed,
                view.rendered_side,
                view.hide_removed,
                side_by_side,
                view.requested_scroll,
                view.per_file,
            )
        })
        .unwrap_or(view.requested_scroll);
    // The strip under the diff follows the line cursor's line and can grow
    // over it: scroll as little as keeps the cursor in sight.
    let requested = match (view.line_cursor, shown) {
        (Some(cursor), Ok(text)) => {
            let row =
                crate::split::row_of(text, view.hide_removed, side_by_side, view.per_file, cursor);
            let bottom = usize::from(requested) + usize::from(body.height.max(1)) - 1;
            match row.checked_sub(bottom) {
                Some(below) if below > 0 => {
                    requested.saturating_add(u16::try_from(below).unwrap_or(u16::MAX))
                }
                _ => requested,
            }
        }
        _ => requested,
    };
    let (limit, effective, horizontal_scroll) =
        if side_by_side && shown.is_ok_and(|text| !text.trim().is_empty()) {
            render_split(
                frame,
                shown.unwrap(),
                view.tint,
                view.word_highlights,
                view.layers.and_then(Result::ok),
                body,
                requested,
                view.horizontal_scroll,
                view.per_file,
                find,
            )
        } else {
            let mut lines = match shown {
                Ok(text) if text.trim().is_empty() => {
                    vec![muted(if view.branch_point && view.label.is_none() {
                        "  no changes since the branch was made"
                    } else {
                        "  no changes"
                    })]
                }
                Ok(text) => patch_lines(
                    text,
                    view.hide_removed,
                    view.tint,
                    view.word_highlights,
                    view.layers.and_then(Result::ok),
                    width,
                    view.per_file,
                    find,
                ),
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
                Paragraph::new(lines).scroll((requested.min(limit), 0)),
                body,
            );
            (limit, requested.min(limit), view.horizontal_scroll)
        };
    if let (Ok(text), Some(turn)) = (shown, view.highlight_diff) {
        highlight_turn(frame, view, text, turn, body, effective, side_by_side);
    }
    if let Ok(text) = shown {
        let lines: Vec<_> = text.lines().collect();
        for index in crate::prompt::marked_rows(text, &view.prompts, view.per_file) {
            let line = lines[index];
            let row =
                crate::split::row_of(text, view.hide_removed, side_by_side, view.per_file, index);
            // Hidden source lines must not put their marker onto a different row.
            if view.hide_removed && line.starts_with('-') {
                continue;
            }
            if let Some(y) = row
                .checked_sub(usize::from(effective))
                .filter(|y| *y < usize::from(body.height))
            {
                let x = if line.starts_with("diff --git ") {
                    body.right().saturating_sub(1)
                } else {
                    body.x
                };
                if body.width > 0 {
                    if let Some(cell) = frame.buffer_mut().cell_mut((x, body.y + y as u16)) {
                        cell.set_symbol("●").set_fg(theme::ACCENT);
                    }
                }
            }
        }
    }
    if let (Some(cursor), Ok(text)) = (view.line_cursor, shown) {
        let row =
            crate::split::row_of(text, view.hide_removed, side_by_side, view.per_file, cursor);
        if let Some(y) = row
            .checked_sub(usize::from(effective))
            .filter(|y| *y < usize::from(body.height))
        {
            let marked = frame
                .buffer_mut()
                .cell((body.x, body.y + y as u16))
                .is_some_and(|cell| cell.symbol() == "●");
            render_line_cursor(frame, Rect::new(body.x, body.y + y as u16, body.width, 1));
            if marked {
                if let Some(cell) = frame.buffer_mut().cell_mut((body.x, body.y + y as u16)) {
                    cell.set_symbol("●");
                }
            }
        }
    }
    if let (Some(area), Some(file)) = (thumbnail_area, selected_file) {
        let file_lines = (!view.worktree.as_os_str().is_empty())
            .then(|| std::fs::File::open(view.worktree.join(file.path)).ok())
            .flatten()
            .and_then(|file| {
                std::io::BufReader::new(file)
                    .lines()
                    .try_fold(0usize, |count, line| line.map(|_| count + 1))
                    .ok()
            });
        render_thumbnail(
            frame,
            file.patch,
            file_lines,
            area,
            view.hide_removed,
            usize::from(effective),
            usize::from(body.height),
            side_by_side,
        );
    }
    if let Some((layers, column)) = legend_area {
        let focused = match (view.line_cursor, layers, shown) {
            (None, Ok(layers), Ok(text)) => {
                let rows = crate::split::source_rows(text, view.hide_removed, side_by_side, false);
                let path = rows
                    .iter()
                    .skip(usize::from(effective))
                    .find_map(|row| match row {
                        Row::Line(line) => {
                            crate::files::position(text, *line).map(|(path, _)| path)
                        }
                        _ => None,
                    });
                files
                    .iter()
                    .find(|(_, file)| Some(file.path) == path)
                    .map(|(_, file)| layers.file_layers(file.patch))
                    .unwrap_or_default()
            }
            _ => touched,
        };
        render_legend(frame, legend_rows(layers, &focused), column);
    }
    if let Some(scroll) = view.help {
        render_shortcut_legend(frame, view, area, scroll);
    }
    CheckoutDiffFeedback {
        side_by_side,
        horizontal_scroll,
        scroll: PreviewFeedback {
            limit,
            effective_scroll: effective,
            viewport: body.height,
        },
        file_list_offset,
    }
}

fn render_shortcut_legend(frame: &mut Frame, view: &CheckoutDiffView<'_>, area: Rect, scroll: u16) {
    let rows: Vec<Line<'_>> = view
        .host_shortcuts
        .iter()
        .copied()
        .chain(
            crate::keys::BINDINGS
                .iter()
                .filter(|binding| {
                    view.report.is_some() || !matches!(binding.label, "E" | "e (per-file view)")
                })
                .map(|binding| (binding.label, binding.description)),
        )
        .map(|(label, description)| {
            Line::from(vec![
                Span::styled(format!("{label}  "), Style::default().fg(theme::ACCENT)),
                Span::raw(description),
            ])
        })
        .collect();
    let block = Block::bordered().title(" Shortcuts · ? / Esc: close · ↑/↓: scroll ");
    let inner = block.inner(area);
    let paragraph = Paragraph::new(rows).wrap(Wrap { trim: false });
    let limit = paragraph
        .line_count(inner.width.max(1))
        .saturating_sub(usize::from(inner.height));
    let scroll = usize::from(scroll).min(limit).min(usize::from(u16::MAX)) as u16;
    frame.render_widget(Clear, area);
    frame.render_widget(paragraph.scroll((scroll, 0)).block(block), area);
}

fn prompt_marker() -> Span<'static> {
    Span::styled(" ●", Style::default().fg(theme::ACCENT))
}

/// Mark the line cursor's row: the editor's cursor-line band across it, and
/// a bar at its edge that stays when the band is over a changed line's own.
/// Found text stays marked over the band, as the current match.
fn render_line_cursor(frame: &mut Frame, row: Rect) {
    let buffer = frame.buffer_mut();
    let found = crate::find::style().bg;
    for x in row.left()..row.right() {
        let Some(cell) = buffer.cell_mut((x, row.y)) else {
            continue;
        };
        if Some(cell.bg) == found {
            cell.set_style(crate::find::current_style());
        } else {
            cell.set_bg(theme::SELECTION_BACKGROUND);
        }
    }
    if let Some(cell) = buffer.cell_mut((row.x, row.y)) {
        cell.set_symbol(LINE_CURSOR_MARKER).set_fg(theme::ACCENT);
    }
}

/// Paint after layout so wrapping, hidden removals and horizontal panning
/// keep their positions. Each side of a split row is attributed separately.
fn highlight_turn(
    frame: &mut Frame,
    view: &CheckoutDiffView<'_>,
    text: &str,
    turn: &str,
    area: Rect,
    scroll: u16,
    split: bool,
) {
    let marked = crate::highlight::marked_rows(text, turn);
    let mut paint = |index: usize, rect: Rect| {
        if !marked.contains(&index) {
            return;
        }
        for x in rect.x..rect.right() {
            if let Some(cell) = frame.buffer_mut().cell_mut((x, rect.y)) {
                if cell.bg != theme::DIFF_WORD_ADDED_BACKGROUND
                    && cell.bg != theme::DIFF_WORD_REMOVED_BACKGROUND
                    && cell.bg != theme::ACCENT
                {
                    cell.set_bg(theme::DIFF_TURN_BACKGROUND);
                }
                cell.modifier.insert(Modifier::BOLD);
            }
        }
    };
    if split {
        let left = area.width / 2;
        for (y, row) in crate::split::aligned(text, view.per_file)
            .into_iter()
            .skip(usize::from(scroll))
            .take(usize::from(area.height))
            .enumerate()
        {
            if let crate::split::SplitRow::Pair(old, new) = row {
                let y = area.y + y as u16;
                if let Some(new) = new {
                    paint(new.index, Rect::new(area.x, y, left, 1));
                }
                if let Some(old) = old {
                    paint(
                        old.index,
                        Rect::new(area.x + left + 1, y, area.width - left - 1, 1),
                    );
                }
            }
        }
    } else {
        for (y, row) in rows(text, view.hide_removed, view.per_file)
            .into_iter()
            .skip(usize::from(scroll))
            .take(usize::from(area.height))
            .enumerate()
        {
            if let Row::Line(index) = row {
                paint(index, Rect::new(area.x, area.y + y as u16, area.width, 1));
            }
        }
    }
}

/// Drawn over the first column of the line cursor's row.
const LINE_CURSOR_MARKER: &str = "▌";

/// Draw aligned rows with fixed gutters and one linked horizontal offset.
fn render_split(
    frame: &mut Frame,
    text: &str,
    tint: Tint,
    word_highlights: bool,
    layers: Option<&Layers>,
    area: Rect,
    scroll: u16,
    pan: u16,
    per_file: bool,
    find: Option<&str>,
) -> (u16, u16, u16) {
    use crate::split::{aligned, Cell, SplitRow};
    let plan = aligned(text, per_file);
    let source_lines: Vec<_> = text.lines().collect();
    let mut rendered = rendered_patch(text, tint, word_highlights, find);
    if let Some(layers) = layers {
        paint_layers(text, &mut rendered, layers);
    }
    let highest = plan
        .iter()
        .flat_map(|row| match row {
            SplitRow::Pair(left, right) => [*left, *right],
            _ => [None, None],
        })
        .flatten()
        .filter_map(|cell| cell.number)
        .max()
        .unwrap_or(0);
    // A patch with no positions at all, such as an agent's edit snippets, is
    // drawn without a number column, as the unified view draws it.
    let gutter = if highest > 0 {
        highest.to_string().len() as u16 + 1
    } else {
        0
    };
    let numbered = |cell: &Cell| cell.number.is_some() && gutter > 0;
    let limit = u16::try_from(plan.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(area.height);
    let effective = scroll.min(limit);
    let left_width = area.width / 2;
    let max_pan = plan
        .iter()
        .flat_map(|row| match row {
            SplitRow::Pair(left, right) => [*left, *right],
            _ => [None, None],
        })
        .flatten()
        .filter_map(|cell| {
            rendered.get(cell.index).map(|line| {
                let old_gutter = if numbered(&cell) {
                    line.spans.first().map(Span::width).unwrap_or(0)
                } else {
                    0
                };
                line.width().saturating_sub(old_gutter)
            })
        })
        .max()
        .unwrap_or(0)
        .saturating_sub(usize::from(
            area.width.saturating_sub(left_width + 1 + gutter),
        ));
    let pan = pan.min(u16::try_from(max_pan).unwrap_or(u16::MAX));
    let draw_cell = |frame: &mut Frame, cell: Option<Cell>, rect: Rect| {
        let Some(cell) = cell else {
            return;
        };
        let Some(mut line) = rendered.get(cell.index).cloned() else {
            return;
        };
        let code = if numbered(&cell) {
            // The unified highlighter emits its line-number gutter as the first span.
            if !line.spans.is_empty() {
                line.spans.remove(0);
            }
            // A hunk without a position, such as an agent's edit snippet
            // (`@@ edit @@`), counts from 0: no line is numbered that.
            let number = match cell.number.unwrap() {
                0 => " ".repeat(usize::from(gutter)),
                number => format!("{number:>width$} ", width = usize::from(gutter - 1)),
            };
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    number,
                    Style::default().fg(theme::INACTIVE).patch(line.style),
                ))),
                Rect::new(rect.x, rect.y, gutter, 1),
            );
            Rect::new(
                rect.x + gutter,
                rect.y,
                rect.width.saturating_sub(gutter),
                1,
            )
        } else {
            rect
        };
        let style = line.style;
        frame.render_widget(Block::default().style(style), code);
        frame.render_widget(Paragraph::new(line).scroll((0, pan)), code);
    };
    for (offset, row) in plan
        .iter()
        .skip(usize::from(effective))
        .take(usize::from(area.height))
        .enumerate()
    {
        let y = area.y + offset as u16;
        match *row {
            SplitRow::Shared(row) => {
                let line = match row {
                    Row::Line(index) => {
                        let line = rendered[index].clone();
                        if source_lines
                            .get(index)
                            .is_some_and(|line| is_file_header(line))
                        {
                            header_bar(line, usize::from(area.width))
                        } else if source_lines
                            .get(index)
                            .is_some_and(|line| is_hunk_header(line))
                        {
                            hunk_rule(line, usize::from(area.width))
                        } else {
                            line
                        }
                    }
                    Row::RuleAbove => rule('▁', usize::from(area.width)),
                    Row::RuleBelow => rule('▔', usize::from(area.width)),
                };
                frame.render_widget(Paragraph::new(line), Rect::new(area.x, y, area.width, 1));
            }
            // The working tree goes on the left and the base on the right.
            SplitRow::Pair(old, new) => {
                draw_cell(frame, new, Rect::new(area.x, y, left_width, 1));
                frame.render_widget(
                    Paragraph::new(muted("│")),
                    Rect::new(area.x + left_width, y, 1, 1),
                );
                draw_cell(
                    frame,
                    old,
                    Rect::new(area.x + left_width + 1, y, area.width - left_width - 1, 1),
                );
            }
        }
    }
    (limit, effective, pan)
}

/// A map of edit positions across the whole file, including unchanged gaps.
/// Removals occupy the left lane and additions the right lane.
fn render_thumbnail(
    frame: &mut Frame,
    patch: &str,
    file_lines: Option<usize>,
    area: Rect,
    hide_removed: bool,
    scroll: usize,
    viewport: usize,
    side_by_side: bool,
) {
    let (extent, positions) = thumbnail_line_positions(patch);
    let total = file_lines.unwrap_or(extent).max(extent).max(1);
    let mut added = 0;
    let mut removed = 0;
    for (line, position) in patch.lines().zip(positions) {
        if position.is_some() {
            match line.as_bytes().first() {
                Some(b'+') => added += 1,
                Some(b'-') => removed += 1,
                _ => {}
            }
        }
    }
    let block = Block::bordered()
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(format!("+{added}"), Style::default().fg(theme::SUCCESS)),
            Span::raw(" "),
            Span::styled(format!("−{removed}"), Style::default().fg(theme::ERROR)),
            Span::raw(" "),
        ]))
        .title_bottom(format!(" {total} lines "))
        .border_style(Style::default().fg(theme::INACTIVE));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    const SECTIONS: usize = 15;
    if inner.height < 15 || inner.width < 2 {
        return;
    }
    let pixels = thumbnail_pixels(patch, file_lines, usize::from(inner.width), SECTIONS);
    let visible = thumbnail_viewport_layout(
        patch,
        file_lines,
        hide_removed,
        scroll,
        viewport,
        SECTIONS,
        side_by_side,
        true,
    );
    for section in 0..SECTIONS {
        let y = inner.y + section as u16;
        for x in 0..inner.width {
            let color = pixels[section * usize::from(inner.width) + usize::from(x)];
            let background = match color {
                theme::ERROR => theme::DIFF_MAP_REMOVED,
                theme::SUCCESS => theme::DIFF_MAP_ADDED,
                _ => theme::SELECTION_BACKGROUND,
            };
            // Background fills touch every edge of the cell, independent of font.
            frame.buffer_mut()[(inner.x + x, y)].reset();
            frame.buffer_mut()[(inner.x + x, y)]
                .set_symbol(" ")
                .set_bg(background);
        }
    }
    render_viewport_box(frame, Rect::new(area.x, inner.y, area.width, 15), &visible);
}

/// Outline the visible range over the fills, preserving their backgrounds.
fn render_viewport_box(frame: &mut Frame, area: Rect, visible: &[bool]) {
    let Some(first) = visible.iter().position(|&visible| visible) else {
        return;
    };
    let last = visible.iter().rposition(|&visible| visible).unwrap();
    let top = area.y
        + if first == last {
            first.saturating_sub(1)
        } else {
            first
        } as u16;
    let bottom = area.y
        + if first == last {
            (last + 1).min(visible.len() - 1)
        } else {
            last
        } as u16;
    let left = area.x;
    let right = area.right() - 1;
    let mut draw = |x, y, glyph| {
        frame.buffer_mut()[(x, y)]
            .set_symbol(glyph)
            .set_fg(theme::DIRECTORY_NAME);
    };
    for x in left + 1..right {
        draw(x, top, "─");
        draw(x, bottom, "─");
    }
    for y in top + 1..bottom {
        draw(left, y, "│");
        draw(right, y, "│");
    }
    draw(left, top, "┌");
    draw(right, top, "┐");
    draw(left, bottom, "└");
    draw(right, bottom, "┘");
}

fn thumbnail_pixels(
    patch: &str,
    file_lines: Option<usize>,
    width: usize,
    height: usize,
) -> Vec<Color> {
    let mut pixels = vec![theme::CODE_BACKGROUND; width * height];
    if width == 0 || height == 0 {
        return pixels;
    }
    let (extent, positions) = thumbnail_line_positions(patch);
    let total = file_lines.unwrap_or(extent).max(extent).max(1);
    let mut counts = vec![[0usize; 2]; height];
    for (line, position) in patch.lines().zip(positions) {
        let Some(position) = position else { continue };
        let lane = match line.as_bytes().first() {
            Some(b'-') => 0,
            Some(b'+') => 1,
            _ => continue,
        };
        counts[(position * height / total).min(height - 1)][lane] += 1;
    }
    // Changed sections fill the width; only the deletion/addition ratio sets
    // the split. Unchanged sections retain their blank background.
    for (section, [removed, added]) in counts.into_iter().enumerate() {
        let changes = removed + added;
        if changes == 0 {
            continue;
        }
        let split = (removed * width + changes / 2) / changes;
        for x in 0..width {
            pixels[section * width + x] = if x < split {
                theme::ERROR
            } else {
                theme::SUCCESS
            };
        }
    }
    pixels
}

/// Map raw patch lines to zero-based positions in the current file. Metadata
/// and synthetic rules have no file position; deletions share their anchor.
fn thumbnail_line_positions(patch: &str) -> (usize, Vec<Option<usize>>) {
    let deleted = patch
        .lines()
        .take_while(|line| !line.starts_with("@@"))
        .any(|line| line.starts_with("deleted file mode ") || line == "+++ /dev/null");
    let mut next = None;
    let mut extent = 0;
    let positions = patch
        .lines()
        .map(|line| {
            if let Some(header) = line.strip_prefix("@@ ") {
                next = header
                    .split_whitespace()
                    .nth(usize::from(!deleted))
                    .and_then(|range| {
                        let range = range.strip_prefix(if deleted { '-' } else { '+' })?;
                        let (start, count) = range.split_once(',').unwrap_or((range, "1"));
                        let start = start.parse::<usize>().ok()?;
                        let count = count.parse::<usize>().ok()?;
                        extent = extent.max(start.saturating_sub(1) + count);
                        Some(start)
                    });
                return None;
            }
            let number = next?;
            match line.as_bytes().first() {
                Some(b'+') | Some(b' ') => {
                    if !deleted {
                        next = Some(number + 1);
                    }
                    Some(number.saturating_sub(1))
                }
                Some(b'-') => {
                    if deleted {
                        next = Some(number + 1);
                    }
                    Some(number.saturating_sub(1))
                }
                _ => None,
            }
        })
        .collect();
    (extent, positions)
}

#[cfg(test)]
fn thumbnail_viewport(
    patch: &str,
    file_lines: Option<usize>,
    hide_removed: bool,
    scroll: usize,
    viewport: usize,
    height: usize,
) -> Vec<bool> {
    thumbnail_viewport_layout(
        patch,
        file_lines,
        hide_removed,
        scroll,
        viewport,
        height,
        false,
        false,
    )
}

fn thumbnail_viewport_layout(
    patch: &str,
    file_lines: Option<usize>,
    hide_removed: bool,
    scroll: usize,
    viewport: usize,
    height: usize,
    side_by_side: bool,
    per_file: bool,
) -> Vec<bool> {
    let mut visible = vec![false; height];
    if height == 0 {
        return visible;
    }
    let (extent, positions) = thumbnail_line_positions(patch);
    let total = file_lines.unwrap_or(extent).max(extent).max(1);
    for row in crate::split::source_rows(patch, hide_removed, side_by_side, per_file)
        .into_iter()
        .skip(scroll)
        .take(viewport)
    {
        if let Row::Line(index) = row {
            if let Some(line) = positions[index] {
                visible[(line * height / total).min(height - 1)] = true;
            }
        }
    }
    visible
}

/// The same change marker beside a filename in the tree and patch title.
fn change_marker(change: FileChange) -> Span<'static> {
    let color = match change {
        FileChange::Added | FileChange::Copied => theme::SUCCESS,
        FileChange::Deleted => theme::ERROR,
        FileChange::Moved => theme::DIRECTORY_NAME,
        _ => theme::WARNING,
    };
    Span::styled(
        format!(" [{}]", change.marker()),
        Style::default().fg(color),
    )
}

/// Keep the selected filename visible while its patch scrolls.
fn file_info(file: &FileDiff<'_>) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {}", file.path),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        change_marker(file.change()),
    ])
}

/// The most of the patch's area a file's description may take, so a long one
/// still leaves the patch in view: one part in this many.
const DESCRIPTION_SHARE: u16 = 2;

/// The edit report's description of `file` across the top of `area`, above
/// a rule; what is left of `area` is the patch's.
fn render_description(
    frame: &mut Frame,
    report: &ReportState,
    file: &FileDiff<'_>,
    area: Rect,
) -> Rect {
    let muted = Style::default().fg(theme::MUTED_TEXT);
    let mut lines = match report.describe(file.path, file.patch) {
        Described::Described { description, stale } => {
            let mut lines = markdown_block_lines_with_links(
                description,
                Style::default().fg(theme::TEXT),
                " ",
                LinkDisplay::Compact,
            );
            if stale {
                lines.push(Line::from(Span::styled(
                    " written before the file's latest edits · E: describe them again",
                    muted,
                )));
            }
            lines
        }
        Described::Missing { .. } if report.pending => Vec::new(),
        Described::Missing { failure: None } => vec![Line::from(Span::styled(
            " the edit report does not describe this file · E: write a new one",
            muted,
        ))],
        Described::Missing {
            failure: Some(failure),
        } => vec![Line::from(Span::styled(
            format!(
                " describing this file failed: {} · E: try again",
                failure.lines().next().unwrap_or_default()
            ),
            Style::default().fg(theme::WARNING),
        ))],
    };
    if report.pending {
        lines.push(Line::from(Span::styled(
            " describing each file's edits…",
            Style::default().fg(theme::ACCENT),
        )));
    }
    let width = usize::from(area.width.saturating_sub(1));
    let lines: Vec<_> = lines
        .into_iter()
        .flat_map(|line| wrap_rendered(line, width, 1))
        .collect();
    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .min(area.height / DESCRIPTION_SHARE)
        .max(1);
    let [top, rest] =
        Layout::vertical([Constraint::Length(height + 1), Constraint::Min(0)]).areas(area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme::INACTIVE)),
        ),
        top,
    );
    rest
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

fn rendered_patch(
    text: &str,
    tint: Tint,
    word_highlights: bool,
    find: Option<&str>,
) -> Vec<Line<'static>> {
    let mut lines = diff_block_lines_fading(text, None, false, tint.fade());
    if word_highlights {
        styra_ui::diff::highlight_words(text, &mut lines);
    }
    if let Some(term) = find {
        crate::find::highlight(text, &mut lines, term);
    }
    // Keep raw metadata available to highlighting and source navigation;
    // only the displayed file separator uses the resolved path.
    let mut files = crate::files::file_diffs(text).into_iter();
    for (index, source) in text.lines().enumerate() {
        if is_file_header(source) {
            if let (Some(file), Some(line)) = (files.next(), lines.get_mut(index)) {
                line.spans = vec![Span::styled(
                    file.path.to_owned(),
                    Style::default().fg(theme::ACCENT),
                )];
            }
        }
    }
    lines
}

/// `text` drawn as its [`rows`], `width` wide. Removed lines are left out
/// after rendering, so line numbers and syntax highlighting still follow the
/// complete patch.
fn patch_lines(
    text: &str,
    hide_removed: bool,
    tint: Tint,
    word_highlights: bool,
    layers: Option<&Layers>,
    width: usize,
    per_file: bool,
    find: Option<&str>,
) -> Vec<Line<'static>> {
    let mut rendered = rendered_patch(text, tint, word_highlights, find);
    let removed: Vec<bool> = if hide_removed {
        rendered.iter().map(is_removed).collect()
    } else {
        Vec::new()
    };
    // Painted once removed rows are known by their own band.
    if let Some(layers) = layers {
        paint_layers(text, &mut rendered, layers);
    }
    // The diff is drawn a row for each of its lines.
    let mut rendered: Vec<_> = rendered.into_iter().map(Some).collect();
    let headers: Vec<bool> = text.lines().map(is_file_header).collect();
    let hunks: Vec<bool> = text.lines().map(is_hunk_header).collect();
    file_view_rows(text, plan(text, &removed), per_file)
        .into_iter()
        .filter_map(|row| match row {
            Row::Line(index) => {
                let line = rendered.get_mut(index)?.take()?;
                Some(if headers[index] {
                    header_bar(line, width)
                } else if hunks[index] {
                    hunk_rule(line, width)
                } else if layers.is_some() {
                    // A layer's tint is no band `fill_changed_rows` knows.
                    band(line, width)
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

/// Tint each changed line of `text`, drawn as `lines`, by the layers that
/// touched it in place of its added or removed band. Word highlights keep
/// their own colours.
fn paint_layers(text: &str, lines: &mut [Line<'static>], layers: &Layers) {
    for ((source, line), marks) in text.lines().zip(lines).zip(layers.marks(text)) {
        let Some(background) = crate::layers::background(&marks, source.starts_with('-')) else {
            continue;
        };
        let band = line.style.bg;
        line.style = line.style.bg(background);
        for span in &mut line.spans {
            if span.style.bg == band {
                span.style = span.style.bg(background);
            }
        }
    }
}

/// A row with a background of its own carried to the edge of the panel.
fn band(mut line: Line<'static>, width: usize) -> Line<'static> {
    let (Some(background), pad) = (line.style.bg, width.saturating_sub(line.width())) else {
        return line;
    };
    if pad > 0 {
        line.spans.push(Span::styled(
            " ".repeat(pad),
            Style::default().bg(background),
        ));
    }
    line
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

/// A hunk's cut with its dashes run on to the edge of the panel, so the gap
/// between hunks reads as a break across the whole diff.
fn hunk_rule(mut line: Line<'static>, width: usize) -> Line<'static> {
    let Some(last) = line.spans.last() else {
        return line;
    };
    let style = last.style;
    // A cut with context keeps a space between its text and the run.
    let gap = if last.content.ends_with('┄') {
        ""
    } else {
        " "
    };
    let pad = width.saturating_sub(line.width() + gap.len());
    if pad > 0 {
        line.spans
            .push(Span::styled(format!("{gap}{}", "┄".repeat(pad)), style));
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
    diff_block_lines(text, None, false)
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
    let mut lines = match view.label {
        Some(label) => supplied_lines(label, view.worktree, strong),
        None => comparison_lines(view, strong),
    };
    // The per-file view says it atop its file list instead.
    if let (Ok(text), false) = (view.diff, view.per_file) {
        lines.push(stat_line(text));
    }
    if let Some(report) = view.report {
        if report.pending {
            lines.push(muted("Describing edits…"));
        }
    }
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

/// The checkout's branch against the branch and commit it is diffed with,
/// and the working tree they are read in.
fn comparison_lines(view: &CheckoutDiffView<'_>, strong: Style) -> Vec<Line<'static>> {
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
    if !view.base_commit.is_empty() {
        let muted = Style::default().fg(theme::MUTED_TEXT);
        comparison.push(Span::styled(" (", muted));
        if view.base_names.is_empty() {
            comparison.push(Span::styled("no branch tip or tag", muted));
        }
        for (position, name) in view.base_names.iter().enumerate() {
            if position > 0 {
                comparison.push(Span::styled(", ", muted));
            }
            let label = if name.starts_with("tag: ") {
                name.clone()
            } else {
                format!("branch tip: {name}")
            };
            comparison.push(Span::styled(label, Style::default().fg(theme::TEXT)));
        }
        comparison.push(Span::styled(")", muted));
    }
    if view.branch_point {
        comparison.push(Span::styled(
            "  (where the branch started)",
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    vec![
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
    ]
}

/// What a supplied diff is, and where its files are opened from, if they are
/// on this host.
fn supplied_lines(label: &str, root: &Path, strong: Style) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(Span::styled(label.to_owned(), strong))];
    if !root.as_os_str().is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                root.display().to_string(),
                Style::default().fg(theme::DIRECTORY_NAME),
            ),
            Span::styled(
                "  · where its files are opened",
                Style::default().fg(theme::MUTED_TEXT),
            ),
        ]));
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

/// Each layer's colour and the commit it is, a row each, oldest first, as
/// overlapping changes are laid one over another; or why there are none.
/// When `touched` names any layers, theirs are the rows that stand out and
/// the others are muted.
fn legend_rows(layers: Result<&Layers, &str>, touched: &[u16]) -> Vec<Line<'static>> {
    let layers = match layers {
        Ok(layers) => layers,
        Err(error) => {
            return vec![Line::from(Span::styled(
                format!("layers: {error}"),
                Style::default().fg(theme::ERROR),
            ))]
        }
    };
    if layers.layers.is_empty() {
        return vec![muted("no commits or uncommitted work to layer")];
    }
    let mut rows = vec![muted("layers, oldest first:")];
    for (index, layer) in layers.layers.iter().enumerate() {
        let index = u16::try_from(index).unwrap_or(u16::MAX);
        let swatch = crate::layers::background(&[index], false).unwrap_or(Color::Reset);
        let (name, text, row) = if touched.contains(&index) {
            let bold = Style::default()
                .bg(theme::SELECTION_BACKGROUND)
                .add_modifier(Modifier::BOLD);
            (
                bold.fg(theme::ACCENT),
                bold.fg(theme::TEXT),
                Style::default().bg(theme::SELECTION_BACKGROUND),
            )
        } else if touched.is_empty() {
            (
                Style::default().fg(theme::ACCENT),
                Style::default().fg(theme::TEXT),
                Style::default(),
            )
        } else {
            let muted = Style::default().fg(theme::MUTED_TEXT);
            (muted, muted, Style::default())
        };
        let mut spans = vec![
            Span::styled("  ", Style::default().bg(swatch)),
            Span::styled(format!(" {}", layer.name), name),
        ];
        if !layer.summary.is_empty() {
            let summary: String = layer.summary.chars().take(LEGEND_SUMMARY).collect();
            let more = if layer.summary.chars().count() > LEGEND_SUMMARY {
                "…"
            } else {
                ""
            };
            spans.push(Span::styled(format!(" {summary}{more}"), text));
        }
        rows.push(Line::from(spans).style(row));
    }
    rows
}

/// The commits that touched the line cursor's line, `mark`, newest first, as
/// they are laid over one another: a row saying which commit each is, then
/// the rest of its message.
fn line_layers(layers: &Layers, mark: &[u16]) -> Vec<Line<'static>> {
    if mark.is_empty() {
        return vec![muted("no commit changed this line")];
    }
    mark.iter()
        .rev()
        .filter_map(|&index| {
            let layer = layers.layers.get(usize::from(index))?;
            let swatch = crate::layers::background(&[index], false).unwrap_or(Color::Reset);
            let mut spans = vec![
                Span::styled("  ", Style::default().bg(swatch)),
                Span::styled(
                    format!(" {}", layer.name),
                    Style::default().fg(theme::ACCENT),
                ),
            ];
            if !layer.date.is_empty() || !layer.author.is_empty() {
                spans.push(Span::styled(
                    format!(" {} · {}", layer.date, layer.author),
                    Style::default().fg(theme::MUTED_TEXT),
                ));
            }
            if !layer.summary.is_empty() {
                spans.push(Span::styled(
                    format!("  {}", layer.summary),
                    Style::default().fg(theme::TEXT),
                ));
            }
            let body = message_body(layer).into_iter().map(|line| {
                // Under the commit's id, past its swatch.
                Line::from(Span::styled(
                    format!("   {line}"),
                    Style::default().fg(theme::MUTED_TEXT),
                ))
            });
            Some(std::iter::once(Line::from(spans)).chain(body))
        })
        .flatten()
        .collect()
}

/// The lines of `layer`'s message shown after its summary, so that the
/// whole message takes at most [`MESSAGE_LINES`]; the last is marked when
/// there is more.
fn message_body(layer: &crate::layers::Layer) -> Vec<String> {
    let lines: Vec<&str> = layer.body.lines().collect();
    let room = MESSAGE_LINES - 1;
    let mut shown: Vec<String> = lines
        .iter()
        .take(room)
        .map(|line| line.to_string())
        .collect();
    if lines.len() > room {
        if let Some(last) = shown.last_mut() {
            last.push_str(" …");
        }
    }
    shown
}

/// The most lines of a commit's message, its summary included, shown for
/// each commit under the line cursor.
const MESSAGE_LINES: usize = 10;

/// How much of a commit's summary the layers' legend shows.
const LEGEND_SUMMARY: usize = 40;

/// Draw the layers' legend in `area`. A row with a background of its own
/// has it to the area's edge.
fn render_legend(frame: &mut Frame, rows: Vec<Line<'static>>, area: Rect) {
    let rows: Vec<_> = rows
        .into_iter()
        .map(|row| band(row, usize::from(area.width)))
        .collect();
    frame.render_widget(Paragraph::new(rows), area);
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

/// As wide as the legend's longest row, with the padding and border the
/// file tree has.
fn legend_width(rows: &[Line<'_>]) -> u16 {
    let longest = rows.iter().map(Line::width).max().unwrap_or(0);
    u16::try_from(longest)
        .unwrap_or(u16::MAX)
        .saturating_add(FILE_TREE_PADDING + 1)
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
    fn shortcut_rows_only_appear_in_the_legend() {
        for help in [None, Some(0), Some(u16::MAX)] {
            let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::bordered());
                    view.diff = Ok(DIFF);
                    view.help = help;
                    render(frame, &view, frame.area());
                })
                .unwrap();
            let output: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if help.is_some() {
                assert!(output.contains("Shortcuts"), "{output}");
                if help == Some(0) {
                    assert!(output.contains("toggle unified / side-by-side"), "{output}");
                } else {
                    assert!(output.contains("close the diff"), "{output}");
                }
            } else {
                assert!(!output.contains("s:"), "{output}");
                assert!(!output.contains("v: edit file"), "{output}");
                assert!(output.contains("fn c()"), "{output}");
            }
        }
    }

    #[test]
    fn file_view_shows_the_title_and_hunks_without_git_headers() {
        for side_by_side in [false, true] {
            let mut terminal = Terminal::new(TestBackend::new(140, 24)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::bordered());
                    view.diff = Ok(DIFF);
                    view.per_file = true;
                    view.side_by_side = side_by_side;
                    let measured = render(frame, &view, frame.area());
                    assert_eq!(measured.side_by_side, side_by_side);
                })
                .unwrap();
            let output: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(output.contains("src/a.rs [M]"), "{output}");
            assert!(output.contains("┄┄┄"), "{output}");
            assert!(output.contains("fn c() {}"), "{output}");
            for metadata in ["diff --git", "index 1111111", "--- a/", "+++ b/"] {
                assert!(!output.contains(metadata), "{output}");
            }
        }
        let (combined, _) = screen(Ok(DIFF), None, 0);
        assert!(combined.contains("src/a.rs"), "{combined}");
        for metadata in ["diff --git", "index 1111111", "--- a/", "+++ b/"] {
            assert!(!combined.contains(metadata), "{combined}");
        }
    }

    #[test]
    fn file_view_retains_changes_without_hunks() {
        for detail in [
            "Binary files a/image.png and b/image.png differ",
            "old mode 100644\nnew mode 100755",
            "similarity index 100%\nrename from old\nrename to new",
        ] {
            let text = format!("diff --git a/old b/new\n{detail}\n");
            let shown = patch_lines(&text, false, Tint::Full, true, None, 80, true, None);
            let shown = shown
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(shown, detail);
        }
    }

    /// With layers, each change is banded across the panel in its layers'
    /// blend instead of green or red, removed rows are still known to hide,
    /// and unchanged rows are left alone.
    #[test]
    fn layers_tint_each_change_by_the_commits_that_made_it() {
        use crate::layers::{background, Layer};
        let layer = |name: &str| Layer {
            name: name.into(),
            ..Layer::default()
        };
        let first = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                     @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n";
        let second = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                      @@ -1,2 +1,3 @@\n fn a() {}\n-fn c() {}\n+fn c() {}\n+fn d() {}\n";
        let layers =
            Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
        let backgrounds = |hide_removed| {
            patch_lines(
                DIFF,
                hide_removed,
                Tint::Full,
                false,
                Some(&layers),
                40,
                false,
                None,
            )
            .into_iter()
            .map(|line| {
                assert!(line.width() == 40 || line.style.bg.is_none());
                line.style.bg
            })
            .collect::<Vec<_>>()
        };
        let (removed, both) = (background(&[0], true), background(&[0, 1], false));
        assert_eq!(
            &backgrounds(false)[3..],
            [None, removed, both, both],
            " fn a, -fn b, +fn c, +fn d"
        );
        assert_eq!(&backgrounds(true)[3..], [None, both, both]);
    }

    /// On a line in line selection, the commits that touched it are listed
    /// at the bottom, newest first; on a line none changed, that is said.
    #[test]
    fn the_line_cursors_commits_are_listed_newest_first_at_the_bottom() {
        use crate::layers::Layer;
        use ratatui::{backend::TestBackend, Terminal};
        let layer = |name: &str, summary: &str| Layer {
            name: name.into(),
            summary: summary.into(),
            author: "Ada".into(),
            date: "2 days ago".into(),
            body: String::new(),
        };
        let first = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                     @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n";
        let second = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                      @@ -1,2 +1,3 @@\n fn a() {}\n-fn c() {}\n+fn c() {}\n+fn d() {}\n";
        let second_layer = Layer {
            body: (1..=12).map(|n| format!("body {n}\n")).collect(),
            ..layer("2222222", "second")
        };
        let layers = Layers::from_steps(&[
            (layer("1111111", "first"), first.into()),
            (second_layer, second.into()),
        ]);
        let bottom = |cursor: usize| {
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::default());
                    view.diff = Ok(DIFF);
                    view.layers = Some(Ok(&layers));
                    view.line_cursor = Some(cursor);
                    render(frame, &view, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            (28..40)
                .map(|y| {
                    (0..100)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        // The diff's side, left of the legend's column.
                        .split('│')
                        .next()
                        .unwrap_or_default()
                        .trim_end()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        // Line 7 is "+fn c() {}", laid by the second commit over the first.
        // The strip is as tall as both commits' messages: a rule, the
        // second commit's row and nine of its body's twelve lines, so ten
        // lines of message, then the first commit's row.
        let rows = bottom(7);
        assert!(rows[0].contains('─'), "{rows:?}");
        assert!(
            rows[1].ends_with("2222222 2 days ago · Ada  second"),
            "{rows:?}"
        );
        assert!(rows[2] == "   body 1", "{rows:?}");
        assert!(rows[10] == "   body 9 …", "{rows:?}");
        assert!(
            rows[11].ends_with("1111111 2 days ago · Ada  first"),
            "{rows:?}"
        );
        // On a line no commit changed, the strip is only as tall as saying so.
        let rows = bottom(5);
        assert!(rows[10].contains('─'), "{rows:?}");
        assert_eq!(rows[11], "no commit changed this line", "{rows:?}");
        assert!(rows[..10].iter().all(|row| !row.contains('─')), "{rows:?}");

        // Where the strip grows over the cursor's line, the diff scrolls to
        // keep it in sight.
        let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
        let mut measured = CheckoutDiffFeedback::default();
        terminal
            .draw(|frame| {
                let mut view = CheckoutDiffView::unread(Block::default());
                view.diff = Ok(DIFF);
                view.layers = Some(Ok(&layers));
                view.line_cursor = Some(7);
                measured = render(frame, &view, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let cursor = (0..14).find(|&y| buffer[(0, y)].symbol() == LINE_CURSOR_MARKER);
        assert!(cursor.is_some(), "the cursor is drawn: {buffer:?}");
        assert!(measured.scroll.effective_scroll > 0);
    }

    /// Files sharing the focused file's or line block's commits stand out
    /// in the file list, including files moved without edits.
    #[test]
    fn the_files_the_cursors_commits_changed_stand_out() {
        use crate::layers::Layer;
        use ratatui::{backend::TestBackend, Terminal};
        let layer = |name: &str| Layer {
            name: name.into(),
            ..Layer::default()
        };
        let moved = "diff --git a/old.txt b/moved.txt\nsimilarity index 100%\nrename from old.txt\nrename to moved.txt\n";
        let text = format!(
            "{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old\n+new\n\
             diff --git a/c.txt b/c.txt\n@@ -1 +1 @@\n-x\n+y\n{moved}"
        );
        let one = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                   @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n\
                   diff --git a/c.txt b/c.txt\n--- a/c.txt\n+++ b/c.txt\n@@ -1 +1 @@\n-x\n+y\n";
        let two = "diff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let layers = Layers::from_steps(&[
            (layer("one"), format!("{one}{moved}")),
            (layer("two"), two.into()),
        ]);
        // The style of each listed file's name, with the cursor on `line` of
        // the patch of the file at `selected` in the diff.
        let names = |selected: usize, line: Option<usize>| {
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::default());
                    view.diff = Ok(&text);
                    view.per_file = true;
                    view.selected_file = selected;
                    view.layers = Some(Ok(&layers));
                    view.line_cursor = line;
                    render(frame, &view, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer().clone();
            ["a.rs", "b.txt", "c.txt", "moved.txt"].map(|name| {
                (0..24)
                    .find_map(|y| {
                        let row: String = (0..50).map(|x| buffer[(x, y)].symbol()).collect();
                        let x = row.find(name)?;
                        let x = u16::try_from(row[..x].chars().count()).unwrap();
                        // Only the file list's rows, not the patch's title.
                        (row.contains('│') && row.find('│')? > row.find(name)?)
                            .then(|| buffer[(x, y)].style())
                    })
                    .unwrap_or_else(|| panic!("{name} listed: {buffer:?}"))
            })
        };
        let standing = |style: Style| style.add_modifier.contains(Modifier::BOLD);
        let muted = |style: Style| style.fg == Some(theme::MUTED_TEXT);

        // On b.txt's "+new", which only the second commit wrote: c.txt and
        // a.rs, only the first's, are muted.
        let [a, _, c, moved] = names(1, Some(3));
        assert!(
            muted(a) && muted(c) && muted(moved),
            "{a:?} {c:?} {moved:?}"
        );
        // On a.rs's "+fn c() {}", the first commit's: c.txt stands out too.
        let [_, b, c, moved] = names(0, Some(7));
        assert!(standing(c) && standing(moved), "{c:?} {moved:?}");
        assert!(muted(b), "{b:?}");
        // Without a line cursor, use all commits contributing to the file.
        let [_, b, c, moved] = names(0, None);
        assert!(
            standing(c) && standing(moved) && muted(b),
            "{b:?} {c:?} {moved:?}"
        );
        // A move without edits also focuses the other files in its commit.
        let [a, b, c, _] = names(3, None);
        assert!(standing(a) && standing(c) && muted(b), "{a:?} {b:?} {c:?}");
        let [a, _, c, moved] = names(1, None);
        assert!(
            muted(a) && muted(c) && muted(moved),
            "{a:?} {c:?} {moved:?}"
        );
        // On an unchanged line, nothing is singled out.
        let [_, b, c, moved] = names(0, Some(5));
        for style in [b, c, moved] {
            assert!(!standing(style) && !muted(style), "{style:?}");
        }
    }

    /// The legend lists the layers a row each: in the per-file view in the
    /// left pane under the file list, and in the combined diff, which has no
    /// such pane, at the top left of the top bar before the header. In
    /// line selection the commits that touched the block of changes under
    /// the cursor stand out, and the others are muted.
    #[test]
    fn the_layers_legend_lists_each_commit_at_the_top_left() {
        use crate::layers::{background, Layer};
        use ratatui::{backend::TestBackend, Terminal};
        let layer = |name: &str, summary: &str| Layer {
            name: name.into(),
            summary: summary.into(),
            ..Layer::default()
        };
        let step = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                    @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n";
        let layers = Layers::from_steps(&[
            (layer("1111111", "first"), step.into()),
            (layer("uncommitted", ""), String::new()),
        ]);
        for (per_file, cursor) in [
            (false, None),
            (true, None),
            (false, Some(8)),
            (false, Some(5)),
        ] {
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::default());
                    view.diff = Ok(DIFF);
                    view.per_file = per_file;
                    view.layers = Some(Ok(&layers));
                    view.line_cursor = cursor;
                    render(frame, &view, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };
            let (top, left) = (0..24)
                .find_map(|y| Some((y, row(y).find("layers, oldest first:")?)))
                .unwrap_or_else(|| panic!("no legend in {buffer:?}"));
            let x = u16::try_from(row(top)[..left].chars().count()).unwrap();
            let border = (0..100)
                .find(|&x| buffer[(x, top)].symbol() == "│")
                .expect("a rule beside the legend");
            let rule = (0..24).find(|&y| row(y).starts_with('─')).unwrap();
            if per_file {
                assert_eq!(x, 0, "in the left pane");
                assert!(top > rule, "below the top bar's rule, at {rule}");
                assert!(
                    row(top - 1).contains("files involved · c:"),
                    "count above the commits"
                );
                assert!(row(top - 2).starts_with('─'), "a rule above the count");
                let above: String = (rule..top)
                    .flat_map(|y| row(y).chars().take(usize::from(border)).collect::<Vec<_>>())
                    .collect();
                assert!(above.contains("a.rs"), "the file list is above: {above}");
            } else {
                assert_eq!((x, top), (0, 0), "at the top left, in the top bar");
                let header: String = row(top).chars().skip(usize::from(border) + 1).collect();
                assert!(header.starts_with(" detached head"), "{header}");
                assert!(
                    rule > top + 2,
                    "the legend is above the bar's rule, at {rule}"
                );
            }
            let at = |y: u16| -> String { (x..100).map(|x| buffer[(x, y)].symbol()).collect() };
            assert!(
                at(top + 1).starts_with("   1111111 first"),
                "{}",
                at(top + 1)
            );
            assert!(at(top + 2).starts_with("   uncommitted"), "{}", at(top + 2));
            for (offset, index) in [(1, 0), (2, 1)] {
                assert_eq!(
                    buffer[(x, top + offset)].bg,
                    background(&[index], false).unwrap()
                );
            }
            let (first, uncommitted) = (&buffer[(x + 3, top + 1)], &buffer[(x + 3, top + 2)]);
            if cursor == Some(8) || cursor.is_none() {
                // "+fn d() {}", in the block "-fn b() {}" "+fn c() {}" begins,
                // which only the first commit touched.
                assert_eq!(first.bg, theme::SELECTION_BACKGROUND);
                assert!(first.modifier.contains(Modifier::BOLD));
                assert_eq!(
                    buffer[(border - 1, top + 1)].bg,
                    theme::SELECTION_BACKGROUND,
                    "banded to the rule"
                );
                assert_eq!(uncommitted.fg, theme::MUTED_TEXT);
                assert_ne!(uncommitted.bg, theme::SELECTION_BACKGROUND);
            } else {
                // On no block, as on " fn a() {}", or with no cursor.
                assert_eq!(first.fg, theme::ACCENT);
                assert_eq!(uncommitted.fg, theme::ACCENT);
                assert_ne!(first.bg, theme::SELECTION_BACKGROUND);
            }
        }
    }

    #[test]
    fn file_focus_highlights_all_its_commits_and_follows_file_changes() {
        use crate::layers::Layer;
        let first = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let second = "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n new\n-before\n+after\n";
        let other = "diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-x\n+y\n";
        let layers = Layers::from_steps(
            &[first, second, other]
                .into_iter()
                .enumerate()
                .map(|(i, patch)| {
                    (
                        Layer {
                            name: format!("commit{i}"),
                            ..Layer::default()
                        },
                        patch.to_owned(),
                    )
                })
                .collect::<Vec<_>>(),
        );
        let text = format!(
            "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n-old\n+new\n-before\n+after\n{other}"
        );
        for selected in [0, 1] {
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::default());
                    view.diff = Ok(&text);
                    view.per_file = true;
                    view.selected_file = selected;
                    view.layers = Some(Ok(&layers));
                    render(frame, &view, frame.area());
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            for index in 0..3 {
                let name = format!("commit{index}");
                let cell = (0..24)
                    .find_map(|y| {
                        let row: String = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
                        row.find(&name).map(|x| &buffer[(x as u16, y)])
                    })
                    .expect("commit listed");
                let highlighted = if selected == 0 { index < 2 } else { index == 2 };
                assert_eq!(
                    cell.bg == theme::SELECTION_BACKGROUND,
                    highlighted,
                    "{name}, file {selected}"
                );
                assert_eq!(cell.modifier.contains(Modifier::BOLD), highlighted);
            }
        }
    }

    #[test]
    fn file_focus_highlights_a_move_without_edits() {
        use crate::layers::Layer;
        let moved = "diff --git a/old.txt b/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
        let other = "diff --git a/other.txt b/other.txt\n@@ -1 +1 @@\n-old\n+new\n";
        let layers = Layers::from_steps(&[
            (
                Layer {
                    name: "move_commit".into(),
                    ..Layer::default()
                },
                moved.into(),
            ),
            (
                Layer {
                    name: "edit_commit".into(),
                    ..Layer::default()
                },
                other.into(),
            ),
        ]);
        let text = format!("{moved}{other}");
        for per_file in [false, true] {
            for files_below in [false, true] {
                let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
                terminal
                    .draw(|frame| {
                        let mut view = CheckoutDiffView::unread(Block::default());
                        view.diff = Ok(&text);
                        view.per_file = per_file;
                        view.files_below = files_below;
                        view.layers = Some(Ok(&layers));
                        render(frame, &view, frame.area());
                    })
                    .unwrap();
                let buffer = terminal.backend().buffer();
                for (name, highlighted) in [("move_commit", true), ("edit_commit", false)] {
                    let cell = (0..24)
                        .find_map(|y| {
                            let row: String = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
                            row.find(name).map(|x| &buffer[(x as u16, y)])
                        })
                        .expect("commit listed");
                    assert_eq!(
                        cell.bg == theme::SELECTION_BACKGROUND,
                        highlighted,
                        "{name}, per_file={per_file}, files_below={files_below}"
                    );
                    assert_eq!(cell.modifier.contains(Modifier::BOLD), highlighted);
                }
            }
        }
    }

    #[test]
    fn hiding_removals_keeps_added_indicators_and_original_line_numbers() {
        let lines = patch_lines(DIFF, true, Tint::Full, true, None, 80, false, None);
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
        assert!(!text.contains("--- a/src/a.rs"), "{text}");
        assert!(text.contains("src/a.rs"), "{text}");
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.style.bg == Some(theme::DIFF_ADDED_BACKGROUND))
                .count(),
            2
        );
        assert!(
            patch_lines(DIFF, false, Tint::Full, true, None, 80, false, None)
                .iter()
                .any(|line| line.style.bg == Some(theme::DIFF_REMOVED_BACKGROUND))
        );
    }

    /// The per-file view's left pane holds the files, then the layers, then
    /// the thumbnail at the bottom; put away, the thumbnail leaves the rest.
    #[test]
    fn the_left_pane_lists_files_then_layers_then_the_thumbnail() {
        use crate::layers::Layer;
        let layer = |name: &str| Layer {
            name: name.into(),
            ..Layer::default()
        };
        let layers = Layers::from_steps(&[(layer("1111111"), String::new())]);
        let draw = |hidden| {
            let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
            terminal
                .draw(|frame| {
                    let mut view = CheckoutDiffView::unread(Block::default());
                    view.diff = Ok(DIFF);
                    view.per_file = true;
                    view.thumbnail = true;
                    view.thumbnail_hidden = hidden;
                    view.layers = Some(Ok(&layers));
                    render(frame, &view, frame.area());
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let first_row = |buffer: &ratatui::buffer::Buffer, text: &str| {
            (0..40).find(|&y| {
                (0..30)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains(text)
            })
        };
        let map = |buffer: &ratatui::buffer::Buffer| {
            (0..40).find(|&y| {
                (0..30).any(|x| {
                    [theme::DIFF_MAP_ADDED, theme::DIFF_MAP_REMOVED].contains(&buffer[(x, y)].bg)
                })
            })
        };
        let shown = draw(false);
        let files = first_row(&shown, "a.rs").expect("the file list");
        let legend = first_row(&shown, "layers, oldest first:").expect("the layers");
        let thumbnail = map(&shown).expect("the thumbnail");
        assert!(
            files < legend && legend < thumbnail,
            "{files} {legend} {thumbnail}"
        );

        let hidden = draw(true);
        assert_eq!(map(&hidden), None, "the thumbnail is put away");
        let legend = first_row(&hidden, "layers, oldest first:").expect("the layers");
        assert!(first_row(&hidden, "a.rs").unwrap() < legend);
    }

    #[test]
    fn fullscreen_thumbnail_follows_the_selected_file_and_fits_beneath_the_tree() {
        let patch = "diff --git a/added.rs b/added.rs\n@@ -0,0 +1 @@\n+    added\n\
                     diff --git a/deleted.rs b/deleted.rs\n@@ -1 +0,0 @@\n-removed\n";
        let draw = |thumbnail, per_file, selected_file, width, height| {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &CheckoutDiffView {
                            diff: Ok(patch),
                            thumbnail,
                            per_file,
                            selected_file,
                            ..CheckoutDiffView::unread(Block::bordered())
                        },
                        frame.area(),
                    );
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let added = draw(true, true, 0, 80, 30);
        let removed = draw(true, true, 1, 80, 30);
        let pixels = |buffer: &ratatui::buffer::Buffer, color| {
            buffer
                .content()
                .iter()
                .filter(|cell| {
                    cell.bg
                        == if color == theme::SUCCESS {
                            theme::DIFF_MAP_ADDED
                        } else {
                            theme::DIFF_MAP_REMOVED
                        }
                })
                .count()
        };
        assert!(pixels(&added, theme::SUCCESS) > 0);
        assert_eq!(pixels(&added, theme::ERROR), 0);
        assert!(pixels(&removed, theme::ERROR) > 0);
        assert_eq!(pixels(&removed, theme::SUCCESS), 0);
        // Image stays in the lower-left, underneath the selected file.
        let (image_x, image_y) = added.pos_of(
            added
                .content()
                .iter()
                .position(|cell| cell.bg == theme::DIFF_MAP_ADDED)
                .unwrap(),
        );
        assert!(image_x < 30 && image_y > 7);
        for buffer in [
            draw(false, true, 0, 80, 30),
            draw(true, false, 0, 80, 30),
            draw(true, true, 0, 20, 10),
        ] {
            let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
            assert!(!output.contains("1 lines"));
        }
    }

    #[test]
    fn fifteen_sections_preserve_edits_inside_the_viewport_outline() {
        let mut terminal = Terminal::new(TestBackend::new(30, 19)).unwrap();
        terminal
            .draw(|frame| {
                render_thumbnail(
                    frame,
                    "@@ -10 +10 @@\n-old\n+new\n@@ -900 +900 @@\n-old\n+new\n",
                    Some(1000),
                    frame.area(),
                    false,
                    0,
                    100,
                    false,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(output.contains("1000 lines"));
        assert!(output.contains("+2") && output.contains("−2"));
        assert!(!output.contains("501"), "sections have no line numbers");
        // All 15 sections are contiguous filled rows, including the viewport rules.
        for y in 1..16 {
            for x in 1..29 {
                assert!([
                    theme::DIFF_MAP_ADDED,
                    theme::DIFF_MAP_REMOVED,
                    theme::SELECTION_BACKGROUND
                ]
                .contains(&buffer[(x, y)].bg));
            }
        }
        let unchanged = &buffer[(2, 9)];
        assert_eq!(unchanged.bg, theme::SELECTION_BACKGROUND);
        assert!(buffer
            .content()
            .iter()
            .any(|cell| cell.bg == theme::DIFF_MAP_ADDED));
        assert!(buffer
            .content()
            .iter()
            .any(|cell| cell.bg == theme::DIFF_MAP_REMOVED));
        assert_eq!(
            buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "┌" && cell.fg == theme::DIRECTORY_NAME)
                .count(),
            1
        );
    }

    #[test]
    fn changed_sections_fill_the_width_and_split_by_edit_ratio() {
        let patch = format!(
            "@@ -1,3 +1,1 @@\n{}+new\n@@ -101,1 +101,3 @@\n-old\n{}@@ -201,0 +201 @@\n+only addition\n@@ -301 +301,0 @@\n-only deletion\n",
            "-old\n".repeat(3), "+new\n".repeat(3)
        );
        let pixels = thumbnail_pixels(&patch, Some(1500), 20, 15);
        for (section, red) in [(0, 15), (1, 5), (2, 0), (3, 20)] {
            let row = &pixels[section * 20..(section + 1) * 20];
            assert!(row[..red].iter().all(|&pixel| pixel == theme::ERROR));
            assert!(row[red..].iter().all(|&pixel| pixel == theme::SUCCESS));
        }
        assert!(pixels[4 * 20..]
            .iter()
            .all(|&pixel| pixel == theme::CODE_BACKGROUND));
    }

    #[test]
    fn deleted_file_uses_original_length_and_positions() {
        let patch = format!(
            "deleted file mode 100644\n+++ /dev/null\n@@ -1,1000 +0,0 @@\n{}",
            "-old\n".repeat(1000)
        );
        let (extent, _) = thumbnail_line_positions(&patch);
        assert_eq!(extent, 1000);
        let pixels = thumbnail_pixels(&patch, None, 2, 10);
        for band in pixels.chunks_exact(2) {
            assert_eq!(band, [theme::ERROR, theme::ERROR]);
        }
    }

    #[test]
    fn viewport_box_moves_with_scrolling_and_keeps_edit_colors() {
        let patch = format!(
            "diff --git a/file.rs b/file.rs\n@@ -10,0 +10,40 @@\n{}@@ -761,0 +801,40 @@\n{}",
            "+early\n".repeat(40),
            "+late\n".repeat(40)
        );
        let draw = |scroll| {
            let mut terminal = Terminal::new(TestBackend::new(80, 30)).unwrap();
            terminal
                .draw(|frame| {
                    render(
                        frame,
                        &CheckoutDiffView {
                            diff: Ok(&patch),
                            per_file: true,
                            thumbnail: true,
                            highlight_diff: None,
                            requested_scroll: scroll,
                            help: None,
                            side_by_side: false,
                            rendered_side: false,
                            horizontal_scroll: 0,
                            ..CheckoutDiffView::unread(Block::bordered())
                        },
                        frame.area(),
                    );
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let top = draw(0);
        let bottom = draw(u16::MAX);
        let markers = |buffer: &ratatui::buffer::Buffer| {
            buffer
                .content()
                .iter()
                .enumerate()
                .filter(|(_, cell)| {
                    ["┌", "└"].contains(&cell.symbol()) && cell.fg == theme::DIRECTORY_NAME
                })
                .map(|(index, _)| buffer.pos_of(index).1)
                .collect::<Vec<_>>()
        };
        let early = markers(&top);
        let late = markers(&bottom);
        assert!(!early.is_empty() && !late.is_empty());
        assert!(early.last().unwrap() < late.first().unwrap());
        for buffer in [&top, &bottom] {
            assert!(buffer
                .content()
                .iter()
                .any(|cell| cell.bg == theme::DIFF_MAP_ADDED));
        }
    }

    #[test]
    fn a_single_box_encloses_disjoint_visible_areas() {
        let mut terminal = Terminal::new(TestBackend::new(12, 17)).unwrap();
        terminal
            .draw(|frame| {
                render_viewport_box(
                    frame,
                    frame.area(),
                    &[false, true, false, false, false, true, false, false],
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        for (position, glyph) in [
            ((0, 1), "┌"),
            ((11, 1), "┐"),
            ((0, 5), "└"),
            ((11, 5), "┘"),
            ((0, 4), "│"),
            ((11, 4), "│"),
            ((5, 1), "─"),
            ((5, 5), "─"),
        ] {
            assert_eq!(buffer[position].symbol(), glyph);
        }
        assert_eq!(
            buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "┌")
                .count(),
            1
        );
        assert_eq!(buffer[(5, 4)].symbol(), " ", "gaps remain inside the box");
    }

    #[test]
    fn viewport_mapping_accounts_for_metadata_rules_and_hidden_removals() {
        let patch = "diff --git a/file b/file\n@@ -10,2 +10,2 @@\n-old\n+new\n same\n@@ -800 +800 @@\n-old\n+new\n";
        // Header, synthetic rule and hunk header have no file position.
        assert!(thumbnail_viewport(patch, Some(1000), false, 0, 3, 10)
            .iter()
            .all(|&visible| !visible));
        let early = thumbnail_viewport(patch, Some(1000), false, 3, 1, 10);
        assert!(early[0]);
        assert_eq!(early, thumbnail_viewport(patch, Some(1000), true, 3, 1, 10));
        let late = thumbnail_viewport(patch, Some(1000), false, 6, 3, 10);
        assert!(late[7]);
        assert_eq!(late.iter().filter(|&&visible| visible).count(), 1);
        assert_eq!(late, thumbnail_viewport(patch, Some(1000), true, 6, 1, 10));
    }

    #[test]
    fn edit_map_places_changes_in_the_whole_file_including_unchanged_gaps_and_tail() {
        let patch = "@@ -10 +10 @@\n-old\n+new\n@@ -801,0 +801,1 @@\n+new\n";
        let pixels = thumbnail_pixels(patch, Some(1000), 10, 10);
        assert_eq!(&pixels[0..5], &[theme::ERROR; 5]);
        assert_eq!(&pixels[5..10], &[theme::SUCCESS; 5]);
        assert!(pixels[10..80]
            .iter()
            .all(|&pixel| pixel == theme::CODE_BACKGROUND));
        assert_eq!(&pixels[80..90], &[theme::SUCCESS; 10]);
        assert!(pixels[90..]
            .iter()
            .all(|&pixel| pixel == theme::CODE_BACKGROUND));
    }

    #[test]
    fn nearby_additions_collapse_to_one_marker_independent_of_code_shape() {
        let one = thumbnail_pixels("@@ -101,0 +101 @@\n+x\n", Some(1000), 10, 10);
        let ten = format!("@@ -101,0 +101,10 @@\n{}", "+    longer code\n".repeat(10));
        assert_eq!(one, thumbnail_pixels(&ten, Some(1000), 10, 10));
        assert!(thumbnail_pixels("Binary files differ", None, 10, 4)
            .iter()
            .all(|&pixel| pixel == theme::CODE_BACKGROUND));
        assert!(thumbnail_pixels(&ten, Some(1000), 0, 4).is_empty());
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
                        host_shortcuts: &[],
                        block: Block::bordered(),
                        worktree: Path::new("/state/worktrees/styra-7"),
                        branch: Some("styra/rename"),
                        base_branch,
                        base_commit: "4bf5c35d0123456789abcdef",
                        base_names: &[],
                        branch_point: true,
                        label: None,
                        diff,
                        highlight_diff: None,
                        requested_scroll: scroll,
                        help: None,
                        side_by_side: false,
                        rendered_side: false,
                        horizontal_scroll: 0,
                        per_file,
                        thumbnail: false,
                        thumbnail_hidden: false,
                        hide_removed: false,
                        compact_dirs: true,
                        files_below: false,
                        tint: Tint::Full,
                        word_highlights: true,
                        layers: None,
                        involved_layers: None,
                        selected_file,
                        file_list_offset: 0,
                        search,
                        report: None,
                        conversation: None,
                        intro: None,
                        line_cursor: None,
                        find: Default::default(),
                        prompts: Vec::new(),
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

    /// The per-file view with `report`, 100 columns wide: the screen, and the
    /// rows left for the patch.
    fn report_screen(report: &ReportState, per_file: bool) -> (String, u16) {
        let (width, height) = (100, 24);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut viewport = 0;
        terminal
            .draw(|frame| {
                let mut view = CheckoutDiffView::unread(Block::bordered());
                view.diff = Ok(DIFF);
                view.per_file = per_file;
                view.report = Some(report);
                viewport = render(frame, &view, frame.area()).scroll.viewport;
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
        (output, viewport)
    }

    /// The selected file's description sits between its name and its patch,
    /// saying when the file has changed since; a report on its way says so,
    /// and a hidden one gives its rows back to the patch.
    #[test]
    fn the_per_file_view_describes_the_selected_file_from_the_report() {
        let (_, without) = report_screen(&ReportState::default(), true);
        let (output, _) = report_screen(&ReportState::default(), true);
        assert!(!output.contains("E:"), "{output}");
        assert!(!output.contains("e: hide"), "{output}");

        let described = |patch: &str| ReportState {
            report: Some(crate::Report {
                files: [(
                    "src/a.rs".to_owned(),
                    crate::FileReport {
                        fingerprint: crate::report::fingerprint(patch),
                        description: "**Replaces** `b` with `c` and `d`, so callers get both."
                            .into(),
                    },
                )]
                .into(),
                failures: Vec::new(),
            }),
            ..Default::default()
        };
        let (output, with) = report_screen(&described(DIFF), true);
        assert!(
            output.contains("Replaces b with c and d, so callers get both."),
            "{output}"
        );
        assert!(!output.contains("written before"), "{output}");
        assert!(!output.contains("e: hide edit report"));
        assert_eq!(with, without - 2, "a line of description and its rule");
        let description = output.find("Replaces b").unwrap();
        assert!(output.find("src/a.rs [M]").unwrap() < description);
        assert!(description < output.find("fn c()").unwrap(), "{output}");

        let (output, _) = report_screen(&described("an earlier patch"), true);
        assert!(output.contains("written before the file's latest edits"));

        let mut pending = described(DIFF);
        pending.pending = true;
        let (output, _) = report_screen(&pending, true);
        assert!(output.contains("Replaces b"), "{output}");
        assert!(output.contains("describing each file's edits…"), "{output}");

        // A file the report is missing because describing it failed says why.
        let mut failed = described(DIFF);
        let report = failed.report.as_mut().unwrap();
        report.files.clear();
        report.failures = vec!["the errand timed out\nafter 180s".into()];
        let (output, _) = report_screen(&failed, true);
        assert!(
            output.contains("describing this file failed: the errand timed out · E: try again"),
            "{output}"
        );

        let mut hidden = described(DIFF);
        hidden.hidden = true;
        let (output, rows) = report_screen(&hidden, true);
        assert!(!output.contains("Replaces b"), "{output}");
        assert!(!output.contains("e: show edit report"), "{output}");
        assert_eq!(rows, without);

        let (output, _) = report_screen(&described(DIFF), false);
        assert!(!output.contains("Replaces b"), "only per file: {output}");
        assert!(!output.contains("f: see it per file"), "{output}");
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
        // The panel border, the widest row ("2 files changed  +3 -2", 22),
        // padding and tree border put the patch at column 26.
        let tree = |row: &&str| row.chars().take(26).collect::<String>();
        let patch = |row: &&str| row.chars().skip(26).collect::<String>();
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
        let stat = rows
            .iter()
            .position(|row| tree(row).contains("2 files changed  +3 -2"))
            .expect(&output);
        assert!(
            tree(&rows[stat + 1]).contains("› b.txt"),
            "atop the file list: {output}"
        );
        assert_eq!(
            rows.iter()
                .filter(|row| row.contains("files changed"))
                .count(),
            1,
            "not in the header too: {output}"
        );
        assert_eq!(viewport, 12, "shortcut rows leave room for the patch");
    }

    /// Below the patch, the files take the full width and no more rows than
    /// FILE_LIST_MAX_HEIGHT, scrolling to keep the selection in sight.
    #[test]
    fn files_below_the_patch_fill_a_strip_of_bounded_height() {
        let text = (0..20)
            .map(|n| format!("diff --git a/f{n:02} b/f{n:02}\n@@ -1 +1 @@\n-old {n}\n+new {n}\n"))
            .collect::<String>();
        let (width, height) = (50, 40);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                let mut view = CheckoutDiffView::unread(Block::bordered());
                view.diff = Ok(&text);
                view.per_file = true;
                view.files_below = true;
                view.thumbnail = true;
                view.selected_file = 19;
                render(frame, &view, frame.area());
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
        let rows = output.lines().collect::<Vec<_>>();
        let stat = rows
            .iter()
            .position(|row| row.contains("20 files changed"))
            .expect(&output);
        // The panel's bottom border closes the strip.
        assert_eq!(
            stat + usize::from(FILE_LIST_MAX_HEIGHT),
            rows.len() - 1,
            "{output}"
        );
        assert!(rows[stat - 1].contains("──────"), "a rule above: {output}");
        assert!(
            rows[stat + 1..].iter().any(|row| row.contains("› f19")),
            "{output}"
        );
        assert!(
            !rows[stat + 1..].iter().any(|row| row.contains("f00")),
            "{output}"
        );
        // The patch has the whole width above it.
        assert!(
            rows[..stat].iter().any(|row| row.contains("new 19")),
            "{output}"
        );
        assert!(
            !rows[..stat].iter().any(|row| row.contains("f18")),
            "{output}"
        );
    }

    #[test]
    fn file_list_and_title_use_the_same_change_markers() {
        let text = "diff --git a/a b/a\nnew file mode 100644\n\
            diff --git a/b b/b\ndeleted file mode 100644\n\
            diff --git a/old b/c\nsimilarity index 100%\nrename from old\nrename to c\n\
            diff --git a/old2 b/d\nsimilarity index 90%\nrename from old2\nrename to d\n@@ -1 +1 @@\n-old\n+new\n";
        let ((output, _), _) = file_screen(Ok(text), Some("main"), 0, true, 0);
        for label in ["a [A]", "b [D]", "c [R]", "d [R+]"] {
            assert!(output.contains(label), "{output}");
        }
        assert!(output.contains("│ a [A]"), "{output}");
        let ((output, _), _) = file_screen(Ok(text), Some("main"), u16::MAX, true, 3);
        assert!(output.contains("│ d [R+]"), "{output}");
        assert!(!output.contains("│ a [A]"), "{output}");
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
        assert!(output.contains("│ c [R]"), "{output}");
        let files = filtered_file_diffs(text, Some("c"));
        let rows = file_tree_rows(&files, true);
        assert_eq!(rows[0].0, Some(2));
        assert_eq!(rows[0].1.to_string(), "c [R]");
        assert_eq!(rows[0].1.spans[1].style.fg, Some(theme::DIRECTORY_NAME));
        for (_, file) in filtered_file_diffs(text, None) {
            let tree = file_tree_rows(
                &[(
                    0,
                    FileDiff {
                        path: file.path,
                        patch: file.patch,
                    },
                )],
                true,
            );
            let title = file_info(&file);
            assert_eq!(title.spans[1], tree[0].1.spans[1]);
        }
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
        let (selected, other) = (row("› b [D]"), row("  a [A]"));
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
        let mut terminal = Terminal::new(TestBackend::new(80, 26)).unwrap();
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
                            host_shortcuts: &[],
                            block: Block::bordered(),
                            worktree: Path::new("/w"),
                            branch: None,
                            base_branch: None,
                            base_commit: "4bf5c35d",
                            base_names: &[],
                            branch_point: true,
                            label: None,
                            diff: Ok(&text),
                            highlight_diff: None,
                            requested_scroll: 0,
                            help: None,
                            per_file: true,
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
                            selected_file,
                            file_list_offset: offset,
                            search: Default::default(),
                            report: None,
                            conversation: None,
                            intro: None,
                            line_cursor: None,
                            find: Default::default(),
                            prompts: Vec::new(),
                        },
                        frame.area(),
                    );
                    assert_eq!(measured.scroll.viewport, 20);
                    offset = measured.file_list_offset;
                })
                .unwrap();
            assert_eq!(offset, expected_offset, "selected file {selected_file}");
            let selection_row = (0..26)
                .find(|&y| terminal.backend().buffer()[(1, y)].symbol() == "›")
                .expect("selection remains visible");
            if (14..=30).contains(&selected_file) {
                // The body spans rows 5 through 24, inclusive.
                assert!(selection_row >= 5 + 5);
                assert!(selection_row <= 24 - 5);
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
            output.contains(
                "styra/rename  against  main at 4bf5c35d01 (no branch tip or tag)  (where"
            ),
            "{output}"
        );
        assert!(output.contains("/state/worktrees/styra-7"), "{output}");
        assert!(output.contains("1 file changed  +2 -1"), "{output}");
        assert!(output.contains("fn c() {}"), "{output}");
    }

    /// A host's conversation takes the header's place; the diff stays.
    #[test]
    fn a_conversation_replaces_the_header() {
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        let frame = terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        branch: Some("styra/rename"),
                        base_commit: "4bf5c35d0123456789abcdef",
                        diff: Ok(DIFF),
                        conversation: Some(vec![Line::from("» rename b to c")]),
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
        assert!(output.contains("» rename b to c"), "{output}");
        assert!(!output.contains("against"), "{output}");
        assert!(!output.contains("file changed"), "{output}");
        assert!(output.contains("fn c() {}"), "{output}");
    }

    /// A supplied diff is called what the host says it is, rather than a
    /// comparison of branches it has none of, and its sides are before and
    /// after rather than a base commit and the working tree.
    #[test]
    fn a_supplied_diff_is_labelled_as_given() {
        let draw = |side_by_side, diff| {
            let mut terminal = Terminal::new(TestBackend::new(100, 16)).unwrap();
            let frame = terminal
                .draw(|frame| {
                    render(
                        frame,
                        &CheckoutDiffView {
                            label: Some("turn 3 · snapshot diff"),
                            diff: Ok(diff),
                            side_by_side,
                            ..CheckoutDiffView::unread(Block::bordered())
                        },
                        frame.area(),
                    );
                })
                .unwrap();
            frame
                .buffer
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };
        let output = draw(false, DIFF);
        assert!(output.contains("turn 3 · snapshot diff"), "{output}");
        assert!(!output.contains("against"), "{output}");
        assert!(!output.contains("working tree"), "{output}");
        assert!(output.contains("1 file changed"), "{output}");
        let output = draw(true, DIFF);
        assert!(output.contains("After"), "{output}");
        assert!(output.contains("│ Before"), "{output}");
        assert!(!output.contains("Base ·"), "{output}");
        let output = draw(false, "");
        assert!(output.contains("no changes"), "{output}");
        assert!(!output.contains("since the branch"), "{output}");
    }

    /// A hunk without a position is drawn side by side without numbers
    /// rather than counted from 0.
    #[test]
    fn a_hunk_without_a_position_has_no_numbers_side_by_side() {
        let mut terminal = Terminal::new(TestBackend::new(120, 16)).unwrap();
        let frame = terminal
            .draw(|frame| {
                render(
                    frame,
                    &CheckoutDiffView {
                        label: Some("turn 1"),
                        diff: Ok("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ edit @@\n-fn old_name\n+fn new_name\n"),
                        side_by_side: true,
                        rendered_side: true,
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
        assert!(output.contains("-fn old_name"), "{output}");
        assert!(!output.contains("0 -fn old_name"), "{output}");
        assert!(!output.contains("0 +fn new_name"), "{output}");
    }

    /// A host's intro sits above the header, which stays, and is held to
    /// half the panel however long it is.
    #[test]
    fn an_intro_goes_above_the_header_and_takes_at_most_half() {
        let draw = |intro: Vec<Line<'static>>| {
            let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
            let frame = terminal
                .draw(|frame| {
                    render(
                        frame,
                        &CheckoutDiffView {
                            label: Some("turn 2"),
                            diff: Ok(DIFF),
                            intro: Some(intro),
                            ..CheckoutDiffView::unread(Block::bordered())
                        },
                        frame.area(),
                    );
                })
                .unwrap();
            let buffer = &frame.buffer;
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
        };
        let rows = draw(vec![Line::from("rename b to c")]);
        let row = |rows: &[String], text: &str| rows.iter().position(|row| row.contains(text));
        assert!(row(&rows, "rename b to c").unwrap() < row(&rows, "turn 2").unwrap());
        let rows = draw((0..40).map(|i| Line::from(format!("line {i}"))).collect());
        assert!(row(&rows, "line 0").is_some());
        assert!(row(&rows, "line 20").is_none(), "{rows:#?}");
        assert!(row(&rows, "turn 2").is_some(), "{rows:#?}");
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
                        host_shortcuts: &[],
                        block: Block::bordered(),
                        worktree: Path::new("/w"),
                        branch: None,
                        base_branch: None,
                        base_commit: "4bf5c35d",
                        base_names: &[],
                        branch_point: true,
                        label: None,
                        diff: Ok(DIFF),
                        highlight_diff: None,
                        requested_scroll: 0,
                        help: None,
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
                        search: Default::default(),
                        report: None,
                        conversation: None,
                        intro: None,
                        line_cursor: None,
                        find: Default::default(),
                        prompts: Vec::new(),
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
        // Each hunk's cut runs the width of the panel.
        let cut = "┄".repeat(78);
        let cut = cut.as_str();
        assert_eq!(
            body(false),
            [
                "[src/a.rs]",
                "▔",
                cut,
                "1  fn a() {}",
                "2 -fn b() {}",
                "2 +fn c() {}",
                "3 +fn d() {}",
                "▁",
                "[b.txt]",
                "▔",
                cut,
                "1 +kept",
                "1 -gone",
                "▁",
                "[c.txt]",
                "▔",
                cut,
                "1 -x",
                "1 +y",
            ]
        );
        let hidden = body(true);
        assert_eq!(hidden.len(), body(false).len() - 3, "{hidden:?}");
        assert_eq!(hidden[10..13], ["1 +kept", "▁", "[c.txt]"]);
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
        assert!(output.contains("no branch tip or tag"), "{output}");
    }

    /// The branches and tags at the base commit follow it, as `git log`
    /// decorates a commit, including the current branch when it is at the base.
    #[test]
    fn the_names_at_the_base_commit_follow_it() {
        let names = [
            "main".to_owned(),
            "styra/rename".into(),
            "tag: v1.2".into(),
            "origin/main".into(),
        ];
        let mut terminal = Terminal::new(TestBackend::new(180, 16)).unwrap();
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
                "against  main at 4bf5c35d01 (branch tip: main, branch tip: styra/rename, tag: v1.2, branch tip: origin/main)  (where the branch started)"
            ),
            "{output}"
        );
    }
}

#[cfg(test)]
mod split_tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn turn_highlights_preserve_search_matches() {
        let patch = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old unique\n+new unique\n";
        for split in [false, true] {
            let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
            let frame = terminal
                .draw(|frame| {
                    let view = CheckoutDiffView {
                        diff: Ok(patch),
                        highlight_diff: Some(patch),
                        side_by_side: split,
                        line_cursor: Some(2),
                        find: SearchView {
                            query: Some("unique"),
                            typing: false,
                        },
                        ..CheckoutDiffView::unread(Block::default())
                    };
                    render(frame, &view, frame.area());
                })
                .unwrap();
            let found = (0..24)
                .find_map(|y| {
                    let row: String = (0..160).map(|x| frame.buffer[(x, y)].symbol()).collect();
                    let start = row.find("new unique")?;
                    let x = row[..start].chars().count() as u16 + 4;
                    Some(&frame.buffer[(x, y)])
                })
                .unwrap();
            // In split view both halves share the cursor's row, whose
            // current search match uses the inverted search style.
            assert_eq!(
                found.bg,
                if split {
                    theme::CODE_BACKGROUND
                } else {
                    theme::ACCENT
                }
            );
            if split {
                assert_eq!(found.fg, theme::LIGHT_ACCENT);
            }
        }
    }

    #[test]
    fn turn_highlights_preserve_all_changes_and_stay_on_the_correct_side() {
        let patch = "diff --git a/a.rs b/a.rs\n@@ -1,2 +1,2 @@\n-old unique\n+new unique\n-other old\n+other new\n";
        let turn = "diff --git a/a.rs b/a.rs\n@@ -40 +40 @@\n-old unique\n+new unique\n";
        for split in [false, true] {
            for per_file in [false, true] {
                for hidden in [false, true] {
                    for enabled in [false, true] {
                        let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
                        let frame = terminal
                            .draw(|frame| {
                                let view = CheckoutDiffView {
                                    diff: Ok(patch),
                                    highlight_diff: enabled.then_some(turn),
                                    side_by_side: split,
                                    per_file,
                                    hide_removed: hidden,
                                    ..CheckoutDiffView::unread(Block::default())
                                };
                                render(frame, &view, frame.area());
                            })
                            .unwrap();
                        for (code, selected) in [
                            ("new unique", true),
                            ("old unique", true),
                            ("other new", false),
                            ("other old", false),
                        ] {
                            let found = (0..32).find_map(|y| {
                                let row: String =
                                    (0..160).map(|x| frame.buffer[(x, y)].symbol()).collect();
                                let byte = row.find(code)?;
                                let x = row[..byte].chars().count() as u16
                                    + code.find(' ').unwrap() as u16
                                    + 1;
                                Some(&frame.buffer[(x, y)])
                            });
                            if hidden && code.contains("old") {
                                assert!(found.is_none());
                            } else {
                                let cell = found.unwrap_or_else(|| panic!("missing {code}"));
                                assert_eq!(
                                    cell.bg == theme::DIFF_TURN_BACKGROUND,
                                    enabled && selected,
                                    "{code}: split={split} per_file={per_file} hidden={hidden}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn split_paints_each_side_and_leaves_missing_cells_neutral() {
        let patch = "diff --git a/a.rs b/a.rs\n@@ -10,2 +20,3 @@\n-old\n+new\n+extra\n keep\n";
        let mut terminal = Terminal::new(TestBackend::new(101, 8)).unwrap();
        terminal
            .draw(|frame| {
                render_split(
                    frame,
                    patch,
                    Tint::Full,
                    true,
                    None,
                    frame.area(),
                    0,
                    0,
                    false,
                    None,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row = |y| {
            (0..101)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        };
        assert!(row(3).starts_with("20 +new"), "{}", row(3));
        assert!(row(3).contains("│10 -old"), "{}", row(3));
        assert!(row(4).starts_with("21 +extra"));
        assert_eq!(buffer[(49, 3)].bg, theme::DIFF_ADDED_BACKGROUND);
        assert_eq!(buffer[(100, 3)].bg, theme::DIFF_REMOVED_BACKGROUND);
        assert_ne!(buffer[(100, 4)].bg, theme::DIFF_REMOVED_BACKGROUND);
        assert!(row(5).starts_with("22  keep"));
        assert!(row(5).contains("│11  keep"));
    }

    #[test]
    fn horizontal_scroll_keeps_gutters_and_divider_fixed() {
        let patch = format!(
            "@@ -123 +456 @@\n-{}tail\n+{}tail\n",
            "x".repeat(80),
            "y".repeat(80)
        );
        let mut terminal = Terminal::new(TestBackend::new(101, 4)).unwrap();
        terminal
            .draw(|frame| {
                render_split(
                    frame,
                    &patch,
                    Tint::Full,
                    true,
                    None,
                    frame.area(),
                    0,
                    32,
                    false,
                    None,
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = (0..101).map(|x| buffer[(x, 1)].symbol()).collect();
        assert!(row.starts_with("456 yyyy"), "{row}");
        assert!(row.contains("│123 xxxx"), "{row}");
        assert_eq!(buffer[(50, 1)].symbol(), "│");
    }

    #[test]
    fn narrow_and_hidden_removal_fallbacks_keep_the_requested_layout() {
        let mut view = CheckoutDiffView::unread(Block::default());
        view.diff = Ok("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n");
        view.side_by_side = true;
        for (width, hidden, expected) in [
            (81, false, true),
            (80, false, false),
            (81, true, false),
            (81, false, true),
            (0, false, false),
        ] {
            view.hide_removed = hidden;
            let mut terminal = Terminal::new(TestBackend::new(width, 25)).unwrap();
            terminal
                .draw(|frame| {
                    let measured = render(frame, &view, frame.area());
                    assert_eq!(measured.side_by_side, expected);
                    let panel = Panel::render(&view, frame, frame.area());
                    assert_eq!(panel.diff_layout.unwrap().0, expected);
                })
                .unwrap();
            assert!(view.side_by_side);
        }
    }
    #[test]
    fn words_work_in_both_layouts_with_tint_and_removals_hidden() {
        let patch = "@@ -1 +1 @@\n-let timeout = 30;\n+let timeout = 60;\n";
        for split in [false, true] {
            for words in [false, true] {
                for tint in [Tint::Full, Tint::Half] {
                    let mut terminal = Terminal::new(TestBackend::new(160, 24)).unwrap();
                    let frame = terminal
                        .draw(|frame| {
                            let view = CheckoutDiffView {
                                diff: Ok(patch),
                                side_by_side: split,
                                word_highlights: words,
                                tint,
                                ..CheckoutDiffView::unread(Block::default())
                            };
                            render(frame, &view, frame.area());
                        })
                        .unwrap();
                    let cells: Vec<_> = frame
                        .buffer
                        .content
                        .iter()
                        .filter(|cell| cell.bg == theme::DIFF_WORD_ADDED_BACKGROUND)
                        .collect();
                    assert_eq!(cells.len(), usize::from(words));
                    if words {
                        assert_eq!(cells[0].symbol(), "6");
                    }
                }
            }
        }
        let lines = patch_lines(patch, true, Tint::Half, true, None, 80, false, None);
        assert!(lines
            .iter()
            .flat_map(|l| &l.spans)
            .any(|s| s.content == "6" && s.style.bg == Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
        assert!(lines
            .iter()
            .flat_map(|l| &l.spans)
            .all(|s| s.style.bg != Some(theme::DIFF_WORD_REMOVED_BACKGROUND)));
    }

    #[test]
    fn panning_keeps_word_highlights_and_gutters_in_place() {
        let padding = "a".repeat(45);
        let patch = format!("@@ -12 +12 @@\n-let {padding} = 30;\n+let {padding} = 60;\n");
        let mut terminal = Terminal::new(TestBackend::new(81, 3)).unwrap();
        let frame = terminal
            .draw(|frame| {
                render_split(
                    frame,
                    &patch,
                    Tint::Full,
                    true,
                    None,
                    frame.area(),
                    0,
                    30,
                    false,
                    None,
                );
            })
            .unwrap();
        let changed: Vec<_> = frame
            .buffer
            .content
            .iter()
            .filter(|c| c.bg == theme::DIFF_WORD_ADDED_BACKGROUND)
            .collect();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].symbol(), "6");
        assert_eq!(frame.buffer[(0, 1)].symbol(), "1");
        assert_eq!(frame.buffer[(1, 1)].symbol(), "2");
        assert_eq!(frame.buffer[(41, 1)].symbol(), "1");
        assert_eq!(frame.buffer[(42, 1)].symbol(), "2");
    }
}
