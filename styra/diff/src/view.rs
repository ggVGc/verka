//! A working tree diffed against a commit, drawn into whatever area the host
//! gives it: a panel inside Styra, or the whole terminal for `styra-diff`.
//!
//! The diff alone does not say what it is a diff *of*, so a header above it
//! names both sides — the branch being worked on and the branch and commit it
//! started from — and the worktree they are read in. The header stays put
//! while the diff below it scrolls.

use crate::files::{filtered_file_diffs, FileChange, FileDiff};
use crate::report::{Described, ReportState};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
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
    /// Show the selected patch thumbnail beneath the file tree in fullscreen.
    pub thumbnail: bool,
    pub hide_removed: bool,
    /// Whether the file tree folds directories with no files of their own
    /// and a single subdirectory onto one row.
    pub compact_dirs: bool,
    pub tint: Tint,
    pub selected_file: usize,
    /// First visible row in the file tree, retained between frames.
    pub file_list_offset: usize,
    pub search: SearchView<'a>,
    /// The edit report the per-file view describes the selected file from.
    /// `None` where no report can be had, as in `styra-diff` on its own,
    /// which leaves its keys out of the header too.
    pub report: Option<&'a ReportState>,
    /// Lines the host shows in place of the header, such as the latest
    /// messages of the conversation the diff belongs to. `None` keeps the
    /// header.
    pub conversation: Option<Vec<Line<'a>>>,
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
            thumbnail: false,
            hide_removed: false,
            compact_dirs: true,
            tint: Tint::Full,
            selected_file: 0,
            file_list_offset: 0,
            search: SearchView::default(),
            report: None,
            conversation: None,
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
    let header = view
        .conversation
        .clone()
        .unwrap_or_else(|| header_lines(view));
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
    let mut thumbnail_area = None;
    let mut file_list_offset = view.file_list_offset;
    let body = if view.per_file {
        let mut rows = file_tree_rows(&files, view.compact_dirs);
        let [left, right] = Layout::horizontal([
            Constraint::Length(if view.thumbnail {
                file_tree_width(&rows, body.width).max(24.min(body.width / 2))
            } else {
                file_tree_width(&rows, body.width)
            }),
            Constraint::Min(0),
        ])
        .areas(body);
        let thumbnail_height =
            if view.thumbnail && selected_file.is_some() && left.width >= 12 && left.height >= 19 {
                17
            } else {
                0
            };
        frame.render_widget(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(Style::default().fg(theme::INACTIVE)),
            left,
        );
        let tree = Rect::new(
            left.x,
            left.y,
            left.width.saturating_sub(1),
            left.height - thumbnail_height,
        );
        if thumbnail_height > 0 {
            thumbnail_area = Some(Rect::new(
                left.x,
                tree.bottom(),
                tree.width.min(30),
                thumbnail_height,
            ));
        }
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
        frame.render_widget(Paragraph::new(file_info(file)), info);
        match view.report.filter(|report| report.shown()) {
            Some(report) => render_description(frame, report, file, patch),
            None => patch,
        }
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
    if let (Some(area), Some(file)) = (thumbnail_area, selected_file) {
        let file_lines = std::fs::File::open(view.worktree.join(file.path))
            .ok()
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
            usize::from(view.requested_scroll.min(limit)),
            usize::from(body.height),
        );
    }
    CheckoutDiffFeedback {
        scroll: PreviewFeedback {
            limit,
            effective_scroll: view.requested_scroll.min(limit),
            viewport: body.height,
        },
        file_list_offset,
    }
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
    let visible = thumbnail_viewport(patch, file_lines, hide_removed, scroll, viewport, SECTIONS);
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

fn thumbnail_viewport(
    patch: &str,
    file_lines: Option<usize>,
    hide_removed: bool,
    scroll: usize,
    viewport: usize,
    height: usize,
) -> Vec<bool> {
    let mut visible = vec![false; height];
    if height == 0 {
        return visible;
    }
    let (extent, positions) = thumbnail_line_positions(patch);
    let total = file_lines.unwrap_or(extent).max(extent).max(1);
    for row in rows(patch, hide_removed)
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
        Described::Missing if report.pending => Vec::new(),
        Described::Missing => vec![Line::from(Span::styled(
            " the edit report does not describe this file · E: write a new one",
            muted,
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
    let mut rendered: Vec<_> = diff_block_lines_fading(text, None, false, tint.fade())
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
    if let Some(report) = view.report {
        let has = report.report.is_some() || report.pending;
        let toggle = match (view.per_file, has, report.hidden) {
            (true, true, false) => " · e: hide edit report",
            (true, true, true) => " · e: show edit report",
            (false, true, _) => " · f: see it per file",
            (_, false, _) => "",
        };
        let state = if report.pending {
            "E: describing edits…"
        } else if report.report.is_some() {
            "E: describe edits again"
        } else {
            "E: describe each file's edits"
        };
        lines.push(muted(&format!("{state}{toggle}")));
    }
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
                            requested_scroll: scroll,
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
                        thumbnail: false,
                        hide_removed: false,
                        compact_dirs: true,
                        tint: Tint::Full,
                        selected_file,
                        file_list_offset: 0,
                        search,
                        report: None,
                        conversation: None,
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
        assert!(output.contains("E: describe each file's edits"), "{output}");
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
            }),
            ..Default::default()
        };
        let (output, with) = report_screen(&described(DIFF), true);
        assert!(
            output.contains("Replaces b with c and d, so callers get both."),
            "{output}"
        );
        assert!(!output.contains("written before"), "{output}");
        assert!(output.contains("E: describe edits again · e: hide edit report"));
        assert_eq!(with, without - 2, "a line of description and its rule");
        let description = output.find("Replaces b").unwrap();
        assert!(output.find("src/a.rs · edited").unwrap() < description);
        assert!(description < output.find("fn c()").unwrap(), "{output}");

        let (output, _) = report_screen(&described("an earlier patch"), true);
        assert!(output.contains("written before the file's latest edits"));

        let mut pending = described(DIFF);
        pending.pending = true;
        let (output, _) = report_screen(&pending, true);
        assert!(output.contains("Replaces b"), "{output}");
        assert!(output.contains("describing each file's edits…"), "{output}");

        let mut hidden = described(DIFF);
        hidden.hidden = true;
        let (output, rows) = report_screen(&hidden, true);
        assert!(!output.contains("Replaces b"), "{output}");
        assert!(output.contains("e: show edit report"), "{output}");
        assert_eq!(rows, without);

        let (output, _) = report_screen(&described(DIFF), false);
        assert!(!output.contains("Replaces b"), "only per file: {output}");
        assert!(output.contains("f: see it per file"), "{output}");
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
                            thumbnail: false,
                            hide_removed: false,
                            compact_dirs: true,
                            tint: Tint::Full,
                            selected_file,
                            file_list_offset: offset,
                            search: Default::default(),
                            report: None,
                            conversation: None,
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
                        thumbnail: false,
                        hide_removed: false,
                        compact_dirs: true,
                        tint: Tint::Full,
                        selected_file: 0,
                        file_list_offset: 0,
                        search: Default::default(),
                        report: None,
                        conversation: None,
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
                "[diff --git a/src/a.rs b/src/a.rs]",
                "▔",
                "index 1111111..2222222 100644",
                "--- a/src/a.rs",
                "+++ b/src/a.rs",
                "@@ -1,2 +1,3 @@",
                "1  fn a() {}",
                "2 -fn b() {}",
                "2 +fn c() {}",
                "3 +fn d() {}",
                "▁",
                "[diff --git a/b.txt b/b.txt]",
                "▔",
                "@@ -1 +1 @@",
                "1 +kept",
                "1 -gone",
                "▁",
                "[diff --git a/c.txt b/c.txt]",
                "▔",
                "@@ -1 +1 @@",
                "1 -x",
                "1 +y",
            ]
        );
        let hidden = body(true);
        assert_eq!(hidden.len(), body(false).len() - 3, "{hidden:?}");
        assert_eq!(
            hidden[13..16],
            ["1 +kept", "▁", "[diff --git a/c.txt b/c.txt]"]
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
