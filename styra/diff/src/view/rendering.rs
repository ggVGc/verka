use super::*;
use crate::files::{filtered_file_diffs, FileChange, FileDiff};
use crate::layers::Layers;
use crate::report::{Described, ReportPage, ReportState};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;
use std::io::BufRead;
use std::path::Path;
use styra_ui::application::{Panel, PanelFeedback};
use styra_ui::diff::{diff_block_lines_fading, fill_changed_rows};
use styra_ui::event_list::wrap_rendered;
use styra_ui::markdown::{markdown_block_lines_with_links, LinkDisplay};
use styra_ui::preview::PreviewFeedback;
use styra_ui::theme;

/// Styra draws the view as its main panel, which `styra-ui` cannot name.
impl Panel for CheckoutDiffView<'_> {
    fn render(&self, frame: &mut Frame, area: Rect) -> PanelFeedback {
        let measured = render(frame, self, area);
        PanelFeedback {
            scroll: measured.scroll,
            list_offset: Some(measured.file_list_offset),
            diff_layout: Some((measured.side_by_side, measured.horizontal_scroll)),
            diff_report_page: measured
                .report_page
                .map(|page| (page.selected, page.scroll, page.preview)),
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
    /// A changed file: its patch's index, name, change kind, and lines
    /// added and removed.
    File(usize, &'a str, FileChange, (usize, usize)),
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
            directory.children.push(Node::File(
                *index,
                name,
                file.change(),
                line_changes(file.patch),
            ));
        }
        root
    }

    fn rows(&self, depth: usize, compact: bool, rows: &mut Vec<(Option<usize>, Line<'a>)>) {
        let indent = "  ".repeat(depth);
        for child in &self.children {
            match child {
                Node::File(index, name, change, (added, removed)) => {
                    // The selection marker takes the indent's last two
                    // columns, so even a file at the top has them.
                    let indent = "  ".repeat(depth.max(1));
                    let mut line = Line::from(vec![
                        Span::raw(indent),
                        Span::raw(*name),
                        change_marker(*change),
                    ]);
                    // Nothing to count for a file with no hunks, such as a
                    // binary one or one only moved.
                    if added + removed > 0 {
                        line.spans.push(Span::raw(" "));
                        line.spans.extend(line_counts(*added, *removed));
                    }
                    rows.push((Some(*index), line));
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

/// Marks the selected file in the tree, in the room every file row's indent
/// leaves for it.
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
    // One more for the border on the right.
    let wanted = u16::try_from(longest)
        .unwrap_or(u16::MAX)
        .saturating_add(FILE_TREE_PADDING + 1);
    wanted.min(available / 2)
}

/// How much of a commit id the header shows: enough to tell commits apart in
/// any repository an operator is likely to have, short enough to read.
const SHORT_COMMIT: usize = 10;

/// Draw the view, and return how far its diff can scroll.
///
/// The host's frame — its title, and the outline that says the view has the
/// keys — is drawn here and only here: everything the view draws, the pages
/// laid over it included, is drawn by [`render_inside`] in what is inside
/// the frame, so nothing can cover it.
pub fn render(frame: &mut Frame, view: &CheckoutDiffView<'_>, area: Rect) -> CheckoutDiffFeedback {
    let block = view
        .chrome
        .as_ref()
        .map(styra_ui::chrome::focused_interaction_block)
        .unwrap_or_else(|| view.block.clone());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    render_inside(frame, view, inner)
}

/// The view inside the host's frame, `area`; [`CheckoutDiffView::block`]
/// is not for drawing here.
fn render_inside(
    frame: &mut Frame,
    view: &CheckoutDiffView<'_>,
    area: Rect,
) -> CheckoutDiffFeedback {
    // The file preview takes the right half, or the bottom half where two
    // columns of code would be too narrow to read; the pages laid over the
    // view still cover all of it.
    let (mut inner, preview_area) = match view.file_preview {
        Some(_) => {
            let halves = [Constraint::Percentage(50), Constraint::Percentage(50)];
            let [diff, preview] = if area.width >= PREVIEW_BESIDE_WIDTH {
                Layout::horizontal(halves).areas(area)
            } else {
                Layout::vertical(halves).areas(area)
            };
            (diff, Some(preview))
        }
        None => (area, None),
    };
    if let Some(commits) = view.commits.filter(|commits| commits.open) {
        let height = (commits.entries.len().saturating_add(3).min(10) as u16).min(inner.height / 3);
        let [list, rest] =
            Layout::vertical([Constraint::Length(height), Constraint::Min(0)]).areas(inner);
        render_commits(frame, commits, list);
        inner = rest;
    }
    let intro = view
        .intro_message
        .as_ref()
        .map(|message| {
            styra_ui::preview::message_lines(message.event, message.protocol, message.links)
        })
        .or_else(|| view.intro.clone());
    if let Some(intro) = &intro {
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
        .conversation_data
        .as_ref()
        .map(|conversation| {
            styra_ui::picker::latest_conversation(
                conversation.events.iter().copied(),
                conversation.protocol,
                conversation.events.len(),
            )
        })
        .or_else(|| view.conversation.clone())
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
    let body = if view.per_file && !view.file_list_hidden {
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
                if let Some(name) = line.spans.get_mut(1) {
                    name.style = if changed {
                        name.style.fg(theme::TEXT).add_modifier(Modifier::BOLD)
                    } else {
                        name.style.fg(theme::MUTED_TEXT)
                    };
                }
            }
        }
        // Only the name: its change marker keeps its own color.
        if let Some(row) = selected {
            let spans = &mut rows[row].1.spans;
            let indent = spans[0].width() - Line::from(SELECTION_MARKER).width();
            spans[0] = Span::raw(format!("{}{SELECTION_MARKER}", " ".repeat(indent)));
            spans[1].style = spans[1].style.fg(theme::SELECTED_FILE_NAME);
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
            );
        let mut state = ListState::default()
            .with_selected(selected)
            .with_offset(file_list_offset);
        frame.render_stateful_widget(list, tree, &mut state);
        file_list_offset = state.offset();
        right
    } else {
        body
    };
    let mut title = None;
    let body = if let Some(file) = selected_file {
        let [info, patch] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
        // Drawn once the scroll is known, with the lines scrolled past.
        title = Some((
            info,
            file_info(file),
            view.prompts.iter().any(|p| p.path == file.path),
        ));
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
                crate::find::match_rows(text, term, view.hidden(), side_by_side, view.per_file)
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
                crate::split::row_of(text, view.hidden(), side_by_side, view.per_file, cursor)
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
                view.hidden(),
                view.rendered_side,
                view.hidden(),
                side_by_side,
                view.requested_scroll,
                view.per_file,
            )
        })
        .unwrap_or(view.requested_scroll);
    // The combined diff pins the header of the file its top row is in over
    // that row; its rows are those `patch_lines` or `render_split` draws.
    let combined_rows = shown.ok().filter(|_| !view.per_file).map(|text| {
        (
            text,
            crate::split::source_rows(text, view.hidden(), side_by_side, false),
        )
    });
    // The strip under the diff follows the line cursor's line and can grow
    // over it: scroll as little as keeps the cursor in sight.
    let requested = match (view.line_cursor, shown) {
        (Some(cursor), Ok(text)) => {
            let row =
                crate::split::row_of(text, view.hidden(), side_by_side, view.per_file, cursor);
            let bottom = usize::from(requested) + usize::from(body.height.max(1)) - 1;
            match row.checked_sub(bottom) {
                Some(below) if below > 0 => {
                    requested.saturating_add(u16::try_from(below).unwrap_or(u16::MAX))
                }
                // Not under the pinned header: scroll up a row to show it.
                _ if row == usize::from(requested)
                    && combined_rows
                        .as_ref()
                        .is_some_and(|(text, rows)| pinned_header(text, rows, row).is_some()) =>
                {
                    requested.saturating_sub(1)
                }
                _ => requested,
            }
        }
        _ => requested,
    };
    let pinned;
    let (limit, effective, horizontal_scroll) =
        if side_by_side && shown.is_ok_and(|text| !text.trim().is_empty()) {
            let drawn = render_split(
                frame,
                shown.unwrap(),
                view.tint,
                view.word_highlights,
                view.layers.and_then(Result::ok),
                body,
                requested,
                view.horizontal_scroll,
                view.hidden(),
                view.per_file,
                find,
            );
            pinned = combined_rows
                .as_ref()
                .and_then(|(text, rows)| pinned_bar(text, rows, usize::from(drawn.1), width));
            drawn
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
                    view.hidden(),
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
            if view.per_file && selected_file.is_some() && shown.is_ok() {
                lines.push(end_of_file(width));
            }
            fill_changed_rows(&mut lines, width);
            let limit = u16::try_from(lines.len())
                .unwrap_or(u16::MAX)
                .saturating_sub(body.height);
            let top = usize::from(requested.min(limit));
            pinned = combined_rows
                .as_ref()
                .and_then(|(text, rows)| pinned_bar(text, rows, top, width));
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
        // Side by side shows removed lines whatever is hidden.
        let hidden = Hidden {
            removed: view.hide_removed && !side_by_side,
            ..view.hidden()
        };
        let hidden_lines = crate::view::hidden_lines(text, hidden);
        for index in crate::prompt::marked_rows(text, &view.prompts, view.per_file) {
            let line = lines[index];
            let row = crate::split::row_of(text, view.hidden(), side_by_side, view.per_file, index);
            // Hidden source lines must not put their marker onto a different row.
            if hidden_lines.get(index).copied().unwrap_or(false) {
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
    if let Some((info, mut label, prompted)) = title {
        let above = match shown {
            Ok(text) => {
                let rows = crate::split::source_rows(text, view.hidden(), side_by_side, true);
                lines_above(text, &rows[..usize::from(effective).min(rows.len())])
            }
            Err(_) => 0,
        };
        label.spans.extend(lines_above_note(above));
        if prompted {
            label.spans.push(prompt_marker());
        }
        frame.render_widget(Paragraph::new(label), info);
    }
    if let Some(header) = pinned.filter(|_| body.height > 1) {
        let row = Rect::new(body.x, body.y, body.width, 1);
        frame.render_widget(Clear, row);
        frame.render_widget(Paragraph::new(header), row);
    }
    if let (Some(cursor), Ok(text)) = (view.line_cursor, shown) {
        let row = crate::split::row_of(text, view.hidden(), side_by_side, view.per_file, cursor);
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
            view.hidden(),
            usize::from(effective),
            usize::from(body.height),
            side_by_side,
        );
    }
    if let Some((layers, column)) = legend_area {
        let focused = match (view.line_cursor, layers, shown) {
            (None, Ok(layers), Ok(text)) => {
                let rows = crate::split::source_rows(text, view.hidden(), side_by_side, false);
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
    if let (Some(file), Some(preview_area)) = (&view.file_preview, preview_area) {
        render_file_preview(frame, file, preview_area);
    }
    let report_page = view
        .report_page
        .map(|page| render_report_page(frame, view, area, page));
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
        report_page,
    }
}

/// The narrowest view the file preview is drawn beside the diff in, rather
/// than below it.
const PREVIEW_BESIDE_WIDTH: u16 = 140;

/// The file the line cursor is in, whole, in `area`, drawn as Styra's
/// preview draws a file a link goes to: the cursor's line marked, and kept
/// in the middle of the preview as far as the file allows.
fn render_file_preview(frame: &mut Frame, file: &FilePreview<'_>, area: Rect) {
    let scroll = match (file.content, file.line) {
        (styra_ui::files::FileContent::Ready(content), Some(line)) => {
            // The rows the lines above it wrap onto, numbered as they are
            // drawn: the marker's column and five digits, then a rule.
            let above: Vec<Line> = content
                .replace('\t', "    ")
                .lines()
                .take(usize::try_from(line.saturating_sub(1)).unwrap_or(usize::MAX))
                .map(|text| Line::raw(format!("{:>6} │ {text}", 0)))
                .collect();
            let inner = Block::bordered().inner(area);
            let row = Paragraph::new(above)
                .wrap(Wrap { trim: false })
                .line_count(inner.width.max(1));
            u16::try_from(row.saturating_sub(usize::from(inner.height / 2))).unwrap_or(u16::MAX)
        }
        _ => 0,
    };
    styra_ui::files::render_content(
        frame,
        Some(file.path),
        file.content,
        file.line,
        scroll,
        area,
    );
}

fn render_commits(frame: &mut Frame, commits: &crate::state::Commits, area: Rect) {
    // Ids and who made each commit when are tinted apart from the
    // summaries, as in the line cursor's legend; muted further while the
    // list recedes.
    let metadata = Style::default().fg(if commits.focused {
        theme::MUTED_TEXT
    } else {
        theme::INACTIVE
    });
    let mut items = vec![ListItem::new("All changes (including uncommitted)")];
    items.extend(commits.entries.iter().map(|commit| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{} ", &commit.id[..7.min(commit.id.len())]),
                Style::default().fg(theme::ACCENT),
            ),
            Span::raw(commit.summary.clone()),
            Span::styled(format!(" · {} · {}", commit.author, commit.date), metadata),
        ]))
    }));
    // Outlined like a panel in Styra's grid: the thick accent border while
    // the list has the keys, a plain inactive one, the list receding, while
    // the patch has them.
    let block = Block::bordered();
    let (block, text, selected) = if commits.focused {
        (
            block
                .title(" Commits · j/k: preview · Enter: patch · Esc: all changes ")
                .border_type(BorderType::Thick)
                .border_style(
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
            theme::TEXT,
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        (
            block
                .title(" Commits · Tab: focus · h: close ")
                .border_style(Style::default().fg(theme::INACTIVE)),
            theme::MUTED_TEXT,
            Style::default().fg(theme::TEXT),
        )
    };
    let widget = List::new(items)
        .style(Style::default().fg(text))
        .block(block)
        .highlight_style(selected)
        .highlight_symbol("› ")
        .highlight_spacing(HighlightSpacing::Always);
    let mut state = ListState::default().with_selected(Some(commits.selected));
    frame.render_stateful_widget(widget, area, &mut state);
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
                    view.report.is_some()
                        || !matches!(binding.label, "E" | "e (per-file view)" | "o")
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

/// Every file of the diff with the edit report's description of it, in the
/// diff's order, on one page drawn over the view, one file selected for
/// opening. Returns where the selection and scroll were held to.
fn render_report_page(
    frame: &mut Frame,
    view: &CheckoutDiffView<'_>,
    area: Rect,
    page: ReportPage,
) -> ReportPage {
    frame.render_widget(Clear, area);
    let (area, below) = match page.preview {
        Some(_) => {
            let [top, bottom] =
                Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .areas(area);
            (top, Some(bottom))
        }
        None => (area, None),
    };
    let block = Block::bordered().title(
        " Edit report · j/k: file · Enter: open it · e: describe it · p: diff below · ↑/↓ PgUp/PgDn: scroll · o / Esc: close ",
    );
    let inner = block.inner(area);
    let width = usize::from(inner.width.saturating_sub(1));
    let height = usize::from(inner.height);
    let wrap = |line| wrap_rendered(line, width, 3);
    let (summary, entries) = report_page_lines(view);
    let mut lines: Vec<_> = summary.into_iter().flat_map(wrap).collect();
    // Each file's rows among `lines`, and its name, which the rows start
    // with: the selected one's is drawn again, marked.
    let mut spans = Vec::new();
    let mut names = Vec::new();
    for (name, description) in entries {
        lines.push(Line::default());
        let start = lines.len();
        lines.extend(wrap(name.clone()));
        lines.extend(description.into_iter().flat_map(wrap));
        spans.push(start..lines.len());
        names.push(name);
    }
    let limit = lines.len().saturating_sub(height);
    let mut scroll = usize::from(page.scroll).min(limit);
    let mut selected = page.selected.min(spans.len().saturating_sub(1));
    if let Some(entry) = spans.get(selected) {
        if page.reveal {
            // The rule above the file too, and the summary above the first.
            let top = if selected == 0 { 0 } else { entry.start - 1 };
            if entry.end > scroll + height {
                scroll = (entry.end - height).min(top);
            }
            scroll = scroll.min(top);
        } else if entry.end <= scroll {
            selected = spans.partition_point(|entry| entry.end <= scroll);
        } else if entry.start >= scroll + height {
            selected = spans
                .partition_point(|entry| entry.start < scroll + height)
                .saturating_sub(1);
        }
    }
    let scroll = scroll.min(limit);
    if let (Some(entry), Some(mut name)) = (spans.get(selected), names.get(selected).cloned()) {
        // Restyled, the name wraps onto as many rows as it did.
        name.spans[1].style = name.spans[1].style.fg(theme::SELECTED_FILE_NAME);
        let marked: Vec<_> = wrap(name);
        let rows = entry.start..entry.start + marked.len();
        lines.splice(rows, marked);
    }
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((u16::try_from(scroll).unwrap_or(u16::MAX), 0))
            .block(block),
        area,
    );
    if let Some(entry) = spans.get(selected) {
        mark_entry(frame, inner, entry.start..entry.end, scroll);
    }
    let preview = page
        .preview
        .zip(below)
        .map(|(preview, below)| render_report_preview(frame, view, selected, below, preview));
    ReportPage {
        selected,
        scroll: u16::try_from(scroll).unwrap_or(u16::MAX),
        reveal: false,
        preview,
    }
}

/// Mark the selected file's entry on the report page — its name and
/// description, the page's rows `rows`, as far as they are in view in
/// `inner` scrolled `scroll` rows down — with a band across it and a bar down
/// its left edge, in the room every entry leaves for the bar.
fn mark_entry(frame: &mut Frame, inner: Rect, rows: std::ops::Range<usize>, scroll: usize) {
    let buffer = frame.buffer_mut();
    let shown = scroll..scroll + usize::from(inner.height);
    for row in rows.start.max(shown.start)..rows.end.min(shown.end) {
        let y = inner.y + u16::try_from(row - scroll).unwrap_or(u16::MAX);
        for x in inner.left()..inner.right() {
            buffer[(x, y)].set_bg(theme::SELECTION_BACKGROUND);
        }
        buffer[(inner.x, y)]
            .set_symbol(ENTRY_MARKER)
            .set_fg(theme::ACCENT);
    }
}

/// Marks the selected entry on the report page, down its left edge.
const ENTRY_MARKER: &str = "▌";

/// The patch of file `selected` of the diff, `scroll` rows down, in `area`
/// below the report page, drawn as the per-file view draws it unified.
/// Returns the scroll it was held to.
fn render_report_preview(
    frame: &mut Frame,
    view: &CheckoutDiffView<'_>,
    selected: usize,
    area: Rect,
    scroll: u16,
) -> u16 {
    let file = view
        .diff
        .ok()
        .and_then(|text| crate::files::file_diffs(text).into_iter().nth(selected));
    // Named, as the page below the list says nothing of which file it is.
    let mut title = vec![Span::raw(" Diff · ")];
    if let Some(file) = &file {
        title.push(Span::styled(
            file.path.to_owned(),
            Style::default()
                .fg(theme::SELECTED_FILE_NAME)
                .add_modifier(Modifier::BOLD),
        ));
        title.push(change_marker(file.change()));
        title.extend(mode_note(file));
        title.push(Span::raw(" · "));
    }
    title.push(Span::raw("J/K: scroll · p: hide "));
    let block = Block::bordered().title(Line::from(title));
    let inner = block.inner(area);
    let width = usize::from(inner.width);
    let mut lines = match file {
        Some(file) => patch_lines(
            file.patch,
            view.hidden(),
            view.tint,
            view.word_highlights,
            view.layers.and_then(Result::ok),
            width,
            true,
            None,
        ),
        None => vec![muted("  no file to show")],
    };
    fill_changed_rows(&mut lines, width);
    let limit = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_sub(inner.height);
    let scroll = scroll.min(limit);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)).block(block), area);
    scroll
}

/// A file on the report page: its name, and its description's lines.
type ReportEntry = (Line<'static>, Vec<Line<'static>>);

/// The report page's lines: what the report covers, then each file's name
/// and description, for every file of the diff once there is a report. A
/// file's name leaves room before it for the selection marker.
fn report_page_lines(view: &CheckoutDiffView<'_>) -> (Vec<Line<'static>>, Vec<ReportEntry>) {
    let muted = Style::default().fg(theme::MUTED_TEXT);
    let note = |text: &str| Line::from(Span::styled(format!(" {text}"), muted));
    let Some(report) = view.report else {
        return (
            vec![note("there is no edit report to write for this diff")],
            Vec::new(),
        );
    };
    let text = match view.diff {
        Ok(text) => text,
        Err(error) => return (vec![note(error)], Vec::new()),
    };
    let files = crate::files::file_diffs(text);
    let described = files
        .iter()
        .filter(|file| {
            matches!(
                report.describe(file.path, file.patch),
                Described::Described { .. }
            )
        })
        .count();
    let summary = vec![match (&report.report, report.pending) {
        (_, true) => Line::from(Span::styled(
            match &report.pending_file {
                Some(path) => format!(" describing {path}'s edits…"),
                None => " describing each file's edits…".to_owned(),
            },
            Style::default().fg(theme::ACCENT),
        )),
        (None, false) => note("no edit report yet · E: write one"),
        (Some(_), false) => note(&format!(
            "{described} of {} files described · E: describe them again",
            files.len()
        )),
    }];
    if report.report.is_none() {
        return (summary, Vec::new());
    }
    let entries = files
        .iter()
        .map(|file| {
            let name = Line::from(vec![
                Span::raw(" ".repeat(Line::from(SELECTION_MARKER).width())),
                Span::styled(
                    file.path.to_owned(),
                    Style::default()
                        .fg(theme::DIRECTORY_NAME)
                        .add_modifier(Modifier::BOLD),
                ),
                change_marker(file.change()),
            ]);
            let mut lines = Vec::new();
            match report.describe(file.path, file.patch) {
                Described::Described { description, stale } => {
                    lines.extend(markdown_block_lines_with_links(
                        description,
                        Style::default().fg(theme::TEXT),
                        "   ",
                        LinkDisplay::Compact,
                    ));
                    if stale {
                        lines.push(note("  written before the file's latest edits"));
                    }
                }
                Described::Missing { .. } if report.describing(file.path) => {}
                Described::Missing { failure: None } => lines.push(note("  not described")),
                Described::Missing {
                    failure: Some(failure),
                } => lines.push(Line::from(Span::styled(
                    format!(
                        "   describing it failed: {}",
                        failure.lines().next().unwrap_or_default()
                    ),
                    Style::default().fg(theme::WARNING),
                ))),
            }
            if report.pending_file.is_some() && report.describing(file.path) {
                lines.push(Line::from(Span::styled(
                    "   describing it again…",
                    Style::default().fg(theme::ACCENT),
                )));
            }
            (name, lines)
        })
        .collect();
    (summary, entries)
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
        for (y, row) in crate::split::aligned(text, view.hidden(), view.per_file)
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
        for (y, row) in rows(text, view.hidden(), view.per_file)
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
    hidden: Hidden,
    per_file: bool,
    find: Option<&str>,
) -> (u16, u16, u16) {
    use crate::split::{aligned, Cell, SplitRow};
    let plan = aligned(text, hidden, per_file);
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
    let limit = u16::try_from(plan.len() + usize::from(per_file))
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
    if per_file {
        if let Some(offset) = plan
            .len()
            .checked_sub(usize::from(effective))
            .filter(|offset| *offset < usize::from(area.height))
        {
            frame.render_widget(
                Paragraph::new(end_of_file(usize::from(area.width))),
                Rect::new(area.x, area.y + offset as u16, area.width, 1),
            );
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
    hidden: Hidden,
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
        hidden,
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
        Hidden {
            removed: hide_removed,
            context: false,
        },
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
    hidden: Hidden,
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
    for row in crate::split::source_rows(patch, hidden, side_by_side, per_file)
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

/// The mode a file the patch adds is given, beside its name in a title, as
/// the patch's own line for it is not drawn.
fn mode_note(file: &FileDiff<'_>) -> Option<Span<'static>> {
    file.new_mode().map(|mode| {
        Span::styled(
            format!(" mode {mode}"),
            Style::default().fg(theme::MUTED_TEXT),
        )
    })
}

/// Keep the selected filename visible while its patch scrolls.
fn file_info(file: &FileDiff<'_>) -> Line<'static> {
    let mut line = Line::from(vec![
        Span::styled(
            format!(" {}", file.path),
            Style::default()
                .fg(theme::TEXT)
                .add_modifier(Modifier::BOLD),
        ),
        change_marker(file.change()),
    ]);
    line.spans.extend(mode_note(file));
    line
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
        Described::Missing { .. } if report.describing(file.path) => Vec::new(),
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
    if report.describing(file.path) {
        lines.push(Line::from(Span::styled(
            if report.pending_file.is_some() {
                " describing this file's edits…"
            } else {
                " describing each file's edits…"
            },
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

/// A file's header line as the diff draws it: its path, and any change of
/// mode.
fn file_title(file: &FileDiff<'_>) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        file.path.to_owned(),
        Style::default().fg(theme::ACCENT),
    )];
    spans.extend(mode_note(file));
    spans
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
                line.spans = file_title(&file);
            }
        }
    }
    lines
}

/// `text` drawn as its [`rows`], `width` wide. Hidden lines are left out
/// after rendering, so line numbers and syntax highlighting still follow the
/// complete patch.
fn patch_lines(
    text: &str,
    hidden: Hidden,
    tint: Tint,
    word_highlights: bool,
    layers: Option<&Layers>,
    width: usize,
    per_file: bool,
    find: Option<&str>,
) -> Vec<Line<'static>> {
    let mut rendered = rendered_patch(text, tint, word_highlights, find);
    let context = if hidden.context {
        crate::view::context_rows(text)
    } else {
        Vec::new()
    };
    let removed = removed_rows(text);
    let left_out: Vec<bool> = (0..rendered.len())
        .map(|index| {
            (hidden.removed && removed.get(index).copied().unwrap_or(false))
                || context.get(index).copied().unwrap_or(false)
        })
        .collect();
    if let Some(layers) = layers {
        paint_layers(text, &mut rendered, layers);
    }
    // The diff is drawn a row for each of its lines.
    let mut rendered: Vec<_> = rendered.into_iter().map(Some).collect();
    let headers: Vec<bool> = text.lines().map(is_file_header).collect();
    let hunks: Vec<bool> = text.lines().map(is_hunk_header).collect();
    file_view_rows(text, plan(text, &left_out), per_file)
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

/// The bar pinned over the top row of the combined diff, `rows` scrolled to
/// `top`: the header of the file that row is in, with how many of the file's
/// lines are out of sight, the row under the bar among them.
fn pinned_bar(text: &str, rows: &[Row], top: usize, width: usize) -> Option<Line<'static>> {
    let row = pinned_header(text, rows, top)?;
    let Row::Line(index) = rows[row] else {
        return None;
    };
    // The file is counted by the headers before its own.
    let nth = text.lines().take(index).filter(|line| is_file_header(line));
    let file = crate::files::file_diffs(text)
        .into_iter()
        .nth(nth.count())?;
    let mut title = file_title(&file);
    title.extend(lines_above_note(lines_above(text, &rows[row + 1..=top])));
    Some(header_bar(Line::from(title), width))
}

/// How many of `rows` are lines of the diff, rather than the cuts between
/// its hunks or the rules around its file headers.
fn lines_above(text: &str, rows: &[Row]) -> usize {
    let lines: Vec<_> = text.lines().collect();
    rows.iter()
        .filter(|row| matches!(row, Row::Line(index) if !is_hunk_header(lines[*index])))
        .count()
}

/// How many lines of a file are scrolled out of sight, after its name.
fn lines_above_note(above: usize) -> Option<Span<'static>> {
    (above > 0).then(|| {
        Span::styled(
            format!("  ↑ {above} {}", if above == 1 { "line" } else { "lines" }),
            Style::default().fg(theme::MUTED_TEXT),
        )
    })
}

/// The row of the header of the file `rows` show at `top`, when it has
/// scrolled above `top`: none while the header is itself on top, nor on the
/// rule that opens the next file.
fn pinned_header(text: &str, rows: &[Row], top: usize) -> Option<usize> {
    if matches!(rows.get(top)?, Row::RuleAbove) {
        return None;
    }
    let lines: Vec<_> = text.lines().collect();
    rows[..=top]
        .iter()
        .rposition(|row| matches!(row, Row::Line(index) if is_file_header(lines[*index])))
        .filter(|&row| row < top)
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

/// A terminator that scrolls with the selected file's content.
fn end_of_file(width: usize) -> Line<'static> {
    let label = "── End of file ";
    Line::styled(
        format!(
            "{label}{}",
            "─".repeat(width.saturating_sub(label.chars().count()))
        ),
        Style::default().fg(theme::MUTED_TEXT),
    )
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
    if let Some(commit) = view.commits.and_then(|commits| {
        commits
            .selected
            .checked_sub(1)
            .and_then(|i| commits.entries.get(i))
    }) {
        lines.push(Line::styled(
            format!(
                "commit {} · {}",
                &commit.id[..7.min(commit.id.len())],
                commit.summary
            ),
            strong,
        ));
    }
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

/// Lines added and removed in the hunks of `text`, one file's patch or a
/// whole diff. Only hunks are counted, so a header's `+++` and `---` are not.
fn line_changes(text: &str) -> (usize, usize) {
    let (mut added, mut removed, mut in_hunk) = (0, 0, false);
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            in_hunk = false;
        } else if line.starts_with("@@") {
            in_hunk = true;
        } else if in_hunk {
            match line.as_bytes().first() {
                Some(b'+') => added += 1,
                Some(b'-') => removed += 1,
                _ => {}
            }
        }
    }
    (added, removed)
}

/// Lines added and removed, as the header and each file in the list show them.
fn line_counts(added: usize, removed: usize) -> [Span<'static>; 3] {
    [
        Span::styled(format!("+{added}"), Style::default().fg(theme::SUCCESS)),
        Span::raw(" "),
        Span::styled(format!("-{removed}"), Style::default().fg(theme::ERROR)),
    ]
}

/// Files changed, how many of them were added, removed, moved and modified,
/// and lines added and removed, counted from the diff itself so the header
/// costs no second `git` run.
fn stat_line(text: &str) -> Line<'static> {
    let (added, removed) = line_changes(text);
    // Added, removed, moved and modified files; a copy is a file added.
    let mut kinds = [0; 4];
    let files = crate::files::file_diffs(text);
    for file in &files {
        kinds[match file.change() {
            FileChange::Added | FileChange::Copied | FileChange::CopiedEdited => 0,
            FileChange::Deleted => 1,
            FileChange::Moved | FileChange::MovedEdited => 2,
            FileChange::Edited | FileChange::Mode => 3,
        }] += 1;
    }
    let breakdown = kinds
        .iter()
        .zip(["added", "removed", "moved", "modified"])
        .filter(|(count, _)| **count > 0)
        .map(|(count, kind)| format!("{count} {kind}"))
        .collect::<Vec<_>>();
    let count = files.len();
    let noun = if count == 1 { "file" } else { "files" };
    let summary = if breakdown.is_empty() {
        format!("{count} {noun} changed  ")
    } else {
        format!("{count} {noun}: {}  ", breakdown.join(", "))
    };
    let mut line = Line::from(Span::styled(summary, Style::default().fg(theme::TEXT)));
    line.spans.extend(line_counts(added, removed));
    line
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
mod tests;

#[cfg(test)]
mod split_tests;

#[cfg(test)]
mod boundary_tests;
