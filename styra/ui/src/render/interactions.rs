use super::*;
impl InteractionState {
    /// The status block an unselected row starts with.
    pub fn block(&self) -> Span<'static> {
        status_block(self.status, false)
    }

    /// The labels an unselected row carries after its name.
    pub fn labels(&self) -> Vec<Span<'static>> {
        state_labels(
            self.status,
            self.stop_reason.as_deref(),
            false,
            self.rate_limited.as_deref(),
            self.newly_idle,
            self.completion,
        )
    }
}

use crate::chrome::StopTone;
use crate::event_list::{blended_style, faded_style};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};
use ratatui::Frame;
use styra_protocol::CompletionState;

/// Three dots, the large one moving rightward one step per event and wrapping
/// back to the left. It fills the whole status block, so a running row reads
/// apart from every other state's single marker, and it holds still when the
/// agent goes quiet.
const RUNNING_INDICATOR: [&str; 3] = ["●··", "·●·", "··●"];

/// The spinner frame for a running turn that has seen `events` events. The
/// navigator row and the conversation's running tail draw the same one.
pub(crate) fn running_indicator(events: usize) -> &'static str {
    RUNNING_INDICATOR[events % RUNNING_INDICATOR.len()]
}

/// How many rows the view keeps between the cursor and its top or bottom edge
/// before it scrolls, where the list has rows there to show.
const SCROLL_MARGIN: usize = 2;

/// How far, in tenths, every interaction but the cursor's is faded toward
/// the background, including its name, status marker and message line.
const UNSELECTED_ROW_FADE: u16 = 2;

/// Additional fading for the supporting text after an unselected row's name.
const UNSELECTED_FADE: u16 = 2;

/// How far, in tenths, the cursor's row has the text after its name lifted
/// toward [`theme::TEXT`].
const SELECTED_BRIGHTEN: u16 = 4;

pub fn height(view: &InteractionNavigator<'_>, available: u16) -> u16 {
    let message_rows = view
        .rows
        .iter()
        .filter(|row| {
            matches!(
                row,
                InteractionRow::Interaction {
                    last_message: Some(_),
                    ..
                }
            )
        })
        .count() as u16;
    (view.rows.len() as u16 + message_rows + 2)
        .max(3)
        .min(available.saturating_div(2).max(3))
        .min(available)
}

pub fn render(frame: &mut Frame, view: &InteractionNavigator<'_>, area: Rect) -> usize {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Thick)
        .border_style(
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        )
        // Scope, completion and tag filters are state the list cannot be
        // read without; the commands that change them are behind `?`.
        .title(format!(
            " {} · interactions · {}{} · ? keys ",
            view.scope,
            view.completion_filter,
            if view.tag_filter.is_empty() {
                String::new()
            } else {
                format!(" · tags: {}", view.tag_filter.join(", "))
            }
        ));
    let block = match view.filter {
        Some(filter) if view.typing_filter || !filter.is_empty() => {
            block.title_bottom(Line::from(vec![
                Span::styled(" /", Style::default().fg(theme::MUTED_TEXT)),
                Span::styled(
                    filter.to_owned(),
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    if view.typing_filter { "▏ " } else { " " },
                    Style::default().fg(theme::ACCENT),
                ),
            ]))
        }
        _ => block,
    };
    let inner = block.inner(area);
    let tree = (0..view.rows.len())
        .map(|index| tree_columns(&view.rows, index))
        .collect::<Vec<_>>();
    let widths = column_widths(view, &tree, inner.width.into());
    // Every other interaction under a heading is drawn on a faint stripe, so
    // the eye can follow a row across the pane and down to its message line.
    // The cursor's row is drawn on a slightly lighter tint in its place.
    let mut striped = false;
    let mut items = view
        .rows
        .iter()
        .zip(&tree)
        .map(|(row, columns)| {
            let mut item = row_item(
                row,
                view.all_workspaces,
                columns,
                inner.width.into(),
                &widths,
            );
            match row {
                InteractionRow::Workspace(_) | InteractionRow::Directory { .. } => striped = false,
                InteractionRow::Interaction {
                    selected, flashing, ..
                } => {
                    let background = if *flashing {
                        Some(theme::FLASHED_ROW_BACKGROUND)
                    } else if *selected {
                        Some(theme::SELECTED_ROW_BACKGROUND)
                    } else {
                        striped.then_some(theme::ALTERNATE_ROW_BACKGROUND)
                    };
                    if let Some(background) = background {
                        for line in &mut item {
                            line.style = line.style.bg(background);
                        }
                    }
                    if !*selected {
                        for line in &mut item {
                            line.style = faded_style(line.style, UNSELECTED_ROW_FADE);
                            for span in &mut line.spans {
                                span.style = faded_style(span.style, UNSELECTED_ROW_FADE);
                            }
                        }
                    }
                    striped = !striped;
                }
            }
            item
        })
        .collect::<Vec<_>>();
    if items.is_empty() {
        if let Some(filter) = view.filter.filter(|filter| !filter.is_empty()) {
            items.push(vec![Line::from(Span::styled(
                format!("  no interaction matches {filter}"),
                Style::default()
                    .fg(theme::MUTED_TEXT)
                    .add_modifier(Modifier::DIM),
            ))]);
        }
    }
    let selected = view
        .rows
        .iter()
        .position(|row| matches!(row, InteractionRow::Interaction { selected: true, .. }));
    // The list is laid out by line rather than by row, so a row with a message
    // line that does not fit is drawn as far as it goes instead of leaving the
    // foot of the pane empty, as `List` would.
    let lines = items.into_iter().flatten().collect::<Vec<_>>();
    let viewport = usize::from(inner.height);
    let offset = scroll_offset(view, selected, viewport, lines.len());
    let above = offset;
    let below = lines.len().saturating_sub(offset + viewport);
    let mut block = block;
    if above > 0 {
        block = block.title(more_lines("↑", above).right_aligned());
    }
    if below > 0 {
        block = block.title_bottom(more_lines("↓", below).right_aligned());
    }
    frame.render_widget(block, area);
    for (row, line) in lines.into_iter().skip(offset).take(viewport).enumerate() {
        let line_area = Rect {
            y: inner.y + row as u16,
            height: 1,
            ..inner
        };
        frame.render_widget(line, line_area);
    }
    // A heavy bar over the left border beside the cursor's row, so the eye
    // finds it from the edge of the pane without anything in the list moving.
    if let (Some(selected), true) = (selected, inner.x > area.x) {
        let start = view.rows[..selected].iter().map(row_height).sum::<usize>();
        let end = start + row_height(&view.rows[selected]);
        let edge = Style::default()
            .fg(theme::SELECTED_ROW_EDGE)
            .add_modifier(Modifier::BOLD);
        for line in start.max(offset)..end.min(offset + viewport) {
            frame.buffer_mut()[(area.x, inner.y + (line - offset) as u16)]
                .set_symbol("┃")
                .set_style(edge);
        }
    }
    offset
}

/// The first line to draw. The view stays where it was while the cursor moves
/// inside it, and scrolls only as far as it takes to keep [`SCROLL_MARGIN`]
/// rows showing past the cursor, fewer where the pane is too short for them.
/// With no selection, show the newest entries at the bottom.
fn scroll_offset(
    view: &InteractionNavigator<'_>,
    selected: Option<usize>,
    viewport: usize,
    total: usize,
) -> usize {
    // The foot of the pane is never left empty while there is list to fill it.
    let last = total.saturating_sub(viewport);
    let mut offset = view.requested_offset.unwrap_or(last).min(last);
    let Some(selected) = selected else {
        return last;
    };
    let lines = |rows: &[InteractionRow<'_>]| rows.iter().map(row_height).sum::<usize>();
    let start = lines(&view.rows[..selected]);
    let end = start + row_height(&view.rows[selected]);
    let mut margin = SCROLL_MARGIN;
    let (above, below) = loop {
        let above = lines(&view.rows[selected.saturating_sub(margin)..selected]);
        let below = lines(&view.rows[selected + 1..(selected + 1 + margin).min(view.rows.len())]);
        if margin == 0 || above + (end - start) + below <= viewport {
            break (above, below);
        }
        margin -= 1;
    };
    // The top edge is applied last, so a row taller than the pane shows its
    // first line rather than its last.
    offset = offset.max((end + below).saturating_sub(viewport));
    offset.min(start - above)
}

/// The note on the pane's border that the list runs on past its edge.
fn more_lines(arrow: &str, count: usize) -> Line<'static> {
    let noun = if count == 1 { "line" } else { "lines" };
    Line::from(Span::styled(
        format!(" {arrow} {count} more {noun} "),
        Style::default().fg(theme::MUTED_TEXT),
    ))
}

fn row_height(row: &InteractionRow<'_>) -> usize {
    match row {
        InteractionRow::Interaction {
            last_message: Some(_),
            ..
        } => 2,
        _ => 1,
    }
}

/// How far in `row` is drawn, and whether it is one of the entries the tree
/// line in its innermost column connects — a heading, or an interaction not
/// hanging beneath another as its branch. `None` for a Workspace heading,
/// which every tree ends at.
fn row_level(row: &InteractionRow<'_>) -> Option<(usize, bool)> {
    match row {
        InteractionRow::Workspace(_) => None,
        InteractionRow::Directory { level, .. } => Some((*level, true)),
        InteractionRow::Interaction { level, depth, .. } => Some((*level, *depth == 0)),
    }
}

/// For each worktree column the row at `index` is drawn in, outermost first,
/// whether the tree line in it carries on past the row: whether another entry
/// connected in that column follows before the heading the column hangs from
/// is left.
fn tree_columns(rows: &[InteractionRow<'_>], index: usize) -> Vec<bool> {
    let Some((level, _)) = row_level(&rows[index]) else {
        return Vec::new();
    };
    (1..=level)
        .map(|column| {
            for row in &rows[index + 1..] {
                match row_level(row) {
                    Some((level, _)) if level < column => return false,
                    Some((level, connected)) if level == column && connected => return true,
                    Some(_) => {}
                    None => return false,
                }
            }
            false
        })
        .collect()
}

/// The tree lines ahead of a row in `columns` — see [`tree_columns`]. A
/// `connected` row is joined to the line in its innermost column; any other
/// line of the row, and every outer column, only carries a line past it.
fn tree_prefix(columns: &[bool], connected: bool) -> String {
    columns
        .iter()
        .enumerate()
        .map(
            |(at, continues)| match (connected && at + 1 == columns.len(), continues) {
                (true, true) => "├─ ",
                (true, false) => "└─ ",
                (false, true) => "│  ",
                (false, false) => "   ",
            },
        )
        .collect()
}

/// How wide the first column of an interaction row is — its tree lines,
/// status block and name — whatever the names in the list. A name that does
/// not fit is cut short, so a long prompt cannot push the other columns out.
const LEAD_COLUMN_WIDTH: usize = 40;

/// How wide each aligned column of the interaction rows is drawn, so the
/// branch, the tags, the model, and the state labels each
/// start at the same place down the list. The first column is a set width —
/// see [`LEAD_COLUMN_WIDTH`]. Each other is as wide as its widest entry, but
/// no wider than its share of the pane. Every cell is held to its column's
/// width — see [`fit_cell`] — so a longer entry is cut short rather than
/// moving the rest of its row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ColumnWidths {
    /// Whether any row is flagged as leaving work uncommitted, so every row
    /// keeps room for the flag and the names start together.
    uncommitted: bool,
    lead: usize,
    tags: usize,
    branch: usize,
    model: usize,
}

/// See [`ColumnWidths`]. `tree` is each row's [`tree_columns`], and `width`
/// the pane's inner width.
fn column_widths(
    view: &InteractionNavigator<'_>,
    tree: &[Vec<bool>],
    width: usize,
) -> ColumnWidths {
    let mut widths = ColumnWidths {
        uncommitted: view.rows.iter().any(|row| {
            matches!(
                row,
                InteractionRow::Interaction {
                    uncommitted: true,
                    ..
                }
            )
        }),
        ..ColumnWidths::default()
    };
    for (row, columns) in view.rows.iter().zip(tree) {
        if let Some(cells) = interaction_cells(row, view.all_workspaces, columns, &widths) {
            widths.tags = widths.tags.max(spans_width(&cells.tags));
            widths.branch = widths.branch.max(spans_width(&cells.branch));
            widths.model = widths.model.max(spans_width(&cells.model));
        }
    }
    // The columns leave room for a label after them, the model taking its
    // share first, and the branch and then the tags what remains.
    let lead = LEAD_COLUMN_WIDTH.min(width / 2);
    let mut room = width.saturating_sub(lead + LABEL_ROOM);
    let mut take = |natural: usize| {
        let taken = natural.min(width / 4).min(room);
        room -= taken;
        taken
    };
    let model = take(widths.model);
    let branch = take(widths.branch.min(BRANCH_COLUMN_LIMIT));
    let tags = take(widths.tags);
    ColumnWidths {
        uncommitted: widths.uncommitted,
        lead,
        tags,
        branch,
        model,
    }
}

/// The widest the branch column grows, the spaces setting it off included. A worktree drawn
/// without a heading has its branch on its row, and an agent's branch is
/// long — it is the target the other worktrees' headings name after `→` — so
/// it is cut short here rather than taking the pane's room from the columns
/// after it. Its start, which says what the work is, still shows.
const BRANCH_COLUMN_LIMIT: usize = 32;

/// How much of the row the aligned columns leave for the labels after them:
/// enough for `  NEWLY IDLE`, so the state a row is in is not pushed off the
/// edge of a narrow pane.
const LABEL_ROOM: usize = 14;

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans
        .iter()
        .map(|span| crate::text::width(&span.content))
        .sum()
}

/// Holds the cell of `spans` from `start` on to exactly `width` cells: cut
/// short with `…` where it is wider, and spaced out where it is narrower. So
/// nothing in one cell can move the cells after it, and every column stays
/// lined up whatever is in it.
fn fit_cell(spans: &mut Vec<Span<'static>>, start: usize, width: usize) {
    let cell = spans.split_off(start);
    let cell = crate::text::truncate(Line::from(cell), width, None);
    let padding = width.saturating_sub(cell.width());
    spans.extend(cell.spans);
    if padding > 0 {
        spans.push(Span::raw(" ".repeat(padding)));
    }
}

/// An interaction row's first line, in the columns [`ColumnWidths`] lines up,
/// before the cursor's row is lifted and the others faded.
struct InteractionCells {
    /// The tree lines, status block and name.
    lead: Vec<Span<'static>>,
    /// Which span of `lead` is the name.
    name_index: usize,
    tags: Vec<Span<'static>>,
    branch: Vec<Span<'static>>,
    model: Vec<Span<'static>>,
    labels: Vec<Span<'static>>,
}

/// `under_workspaces` says whether the list is drawn under Workspace
/// headings, which every row then steps in beneath. `columns` are the
/// worktree headings it is drawn beneath — see [`tree_columns`]. `width` is
/// the pane's inner width, which a Workspace heading's rule runs out to.
/// `widths` lines the interaction rows' columns up.
fn row_item(
    row: &InteractionRow<'_>,
    under_workspaces: bool,
    columns: &[bool],
    width: usize,
    widths: &ColumnWidths,
) -> Vec<Line<'static>> {
    // Bright, undimmed tree lines make membership readable independently of
    // heading colors, including when the heading has scrolled out of view.
    let group_style = Style::default()
        .fg(theme::TEXT)
        .add_modifier(Modifier::BOLD);
    let workspace_edge = if under_workspaces { " " } else { "" };
    match row {
        InteractionRow::Workspace(name) => {
            // A faint rule from the name to the pane's edge sets each
            // workspace's rows off from the ones above.
            let fill = width.saturating_sub(crate::text::width(&name) + 1);
            let mut spans = vec![Span::styled(
                name.to_string(),
                Style::default()
                    .fg(theme::WORKSPACE_NAME)
                    .add_modifier(Modifier::BOLD),
            )];
            if fill > 0 {
                spans.push(Span::styled(
                    format!(" {}", "─".repeat(fill)),
                    Style::default().fg(theme::WORKSPACE_RULE),
                ));
            }
            return vec![Line::from(spans)];
        }
        InteractionRow::Directory { name, into, .. } => {
            let mut spans = vec![
                Span::styled(
                    format!("{workspace_edge}{}", tree_prefix(columns, true)),
                    group_style,
                ),
                Span::styled(
                    format!("[{name}]"),
                    Style::default().fg(theme::DIRECTORY_NAME),
                ),
            ];
            // Where its work goes, quietly: the nesting already says so for a
            // worktree under another's heading, but not for one whose target
            // is not listed, nor once the heading above has scrolled away.
            // The target starts where the rows' branches do, in the gap
            // before the branch column, so it reads down the list with them.
            if let Some(into) = into {
                fit_cell(&mut spans, 0, widths.lead.saturating_sub(1));
                spans.push(Span::styled(
                    format!(" → {into}"),
                    Style::default().fg(theme::MUTED_TEXT),
                ));
            }
            return vec![Line::from(spans)];
        }
        InteractionRow::Interaction { .. } => {}
    }
    let InteractionRow::Interaction {
        depth,
        selected,
        last_message,
        ..
    } = row
    else {
        unreachable!()
    };
    let Some(cells) = interaction_cells(row, under_workspaces, columns, widths) else {
        unreachable!()
    };
    let InteractionCells {
        lead: mut main,
        name_index,
        tags,
        branch,
        model,
        labels,
        ..
    } = cells;
    // The name, and the source after it, end the lead, so they are what a
    // long lead loses.
    fit_cell(&mut main, 0, widths.lead);
    for (cell, width) in [
        (branch, widths.branch),
        (tags, widths.tags),
        (model, widths.model),
    ] {
        let start = main.len();
        main.extend(cell);
        fit_cell(&mut main, start, width);
    }
    main.extend(labels);
    // No trailing spaces where nothing follows to be lined up.
    while main.len() > name_index + 1
        && main
            .last()
            .is_some_and(|span| span.content.trim().is_empty())
    {
        main.pop();
    }
    // Everything after the name is lifted toward white and bolded on the
    // cursor's row, and faded on every other, so the cursor's row stands out.
    // The tree edges, status marker and name keep their base styles here;
    // render applies the whole-row fade to unselected interactions afterward.
    let message_style = Style::default().fg(LAST_MESSAGE_COLOR);
    let adjust = |style: Style| {
        if *selected {
            blended_style(style, theme::TEXT, SELECTED_BRIGHTEN).add_modifier(Modifier::BOLD)
        } else {
            faded_style(style, UNSELECTED_FADE)
        }
    };
    for span in main.iter_mut().skip(name_index + 1) {
        span.style = adjust(span.style);
    }
    let message_style = adjust(message_style);
    let edge = branch_indent(*depth);
    let mut lines = vec![Line::from(main)];
    if let Some(text) = last_message {
        let directory_continuation = tree_prefix(columns, false);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{workspace_edge}{directory_continuation}"),
                group_style,
            ),
            Span::styled(
                format!(
                    "{}    {}",
                    " ".repeat(edge.chars().count()),
                    last_message_text(text)
                ),
                message_style,
            ),
        ]));
    }
    lines
}

/// The columns of an interaction row's first line — `None` for a heading.
/// See [`row_item`] for the arguments.
fn interaction_cells(
    row: &InteractionRow<'_>,
    under_workspaces: bool,
    columns: &[bool],
    widths: &ColumnWidths,
) -> Option<InteractionCells> {
    let InteractionRow::Interaction {
        name,
        level,
        depth,
        branched_from,
        branches,
        branch_link,
        model,
        branch,
        expected_branch,
        status,
        selected,
        loading,
        newly_idle,
        stop_reason,
        rate_limited,
        uncommitted,
        completion,
        tags,
        ..
    } = row
    else {
        return None;
    };
    let group_style = Style::default()
        .fg(theme::TEXT)
        .add_modifier(Modifier::BOLD);
    let workspace_edge = if under_workspaces { " " } else { "" };
    // A branch hangs from its source's row rather than joining the worktree's
    // line itself, so the line only carries on past it.
    let directory_edge = tree_prefix(columns, *depth == 0);
    let edge = branch_indent(*depth);
    let mut main = vec![
        Span::styled(format!("{workspace_edge}{directory_edge}"), group_style),
        Span::styled(edge.clone(), Style::default().fg(theme::INACTIVE)),
        status_block(*status, *selected),
        Span::raw(" "),
    ];
    // Work left uncommitted is flagged ahead of the prompt, where the eye
    // starts on every row, rather than at the end of a line of labels.
    if *uncommitted {
        main.push(Span::styled(
            "! ",
            Style::default()
                .fg(theme::UNCOMMITTED)
                .add_modifier(Modifier::BOLD),
        ));
    } else if widths.uncommitted {
        main.push(Span::raw("  "));
    }
    // The prompt stays brighter than the gray message line beneath it. The
    // cursor's prompt is bold and in its own color, whatever its status.
    let name_style = if *selected {
        Style::default()
            .fg(theme::SELECTED_LIVE_INTERACTION_TEXT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::TEXT)
    };
    // The source of the cursor's branch, and the cursor's own branches, are
    // underlined, so a branch drawn under another checkout than its source's
    // can still be traced to it, from either end.
    let name_style = if *branch_link != BranchLink::None {
        name_style.add_modifier(Modifier::UNDERLINED)
    } else {
        name_style
    };
    // Where the conversation came from, and went, when the rows are not
    // nested to say so — the tree is laid out by where the work goes — as an
    // icon ahead of the name: `↳` for a branch of another interaction, `↱N`
    // for one with N branches drawn elsewhere. Plain arrows rather than `⑂`,
    // which few terminal fonts draw. The icon starts the name itself, so a
    // row without one keeps no room for it. An icon is lit when the cursor
    // is at either end of the branch it stands for, telling an underlined
    // source from an underlined branch.
    let fork_style = |lit: bool| {
        Style::default().fg(if lit {
            theme::BRANCH_LINK
        } else {
            theme::MUTED_TEXT
        })
    };
    let mut fork = Vec::new();
    if branched_from.is_some() {
        let lit = *selected || *branch_link == BranchLink::Branch;
        fork.push(Span::styled("↳", fork_style(lit)));
    }
    if *branches > 0 {
        let lit = *selected || *branch_link == BranchLink::Source;
        fork.push(Span::styled(format!("↱{branches}"), fork_style(lit)));
    }
    if !fork.is_empty() {
        main.extend(fork);
        main.push(Span::raw(" "));
    }
    let name_index = main.len();
    // A prompt's line breaks and tabs are not drawn, which would run its
    // words together; they read as the spaces they stand for.
    let name = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>();
    main.push(Span::styled(name, name_style));
    // The source is named after the name, in the first column, where it is
    // the first thing a long row gives up.
    if let Some(source) = branched_from {
        main.push(Span::styled(
            format!(" from {source}"),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    // Set off from the branch before them like every other column.
    let mut tag_cell = Vec::new();
    if !tags.is_empty() {
        tag_cell.push(Span::styled(
            format!("  #{}", tags.join(" #")),
            Style::default().fg(theme::INTERACTION_TAG),
        ));
    }
    let mut branch_cell = Vec::new();
    // Worktree rows leave the branch to their heading.
    if let Some(branch) = branch.filter(|_| *level == 0 && expected_branch.is_none()) {
        branch_cell.push(Span::styled(
            format!("  {branch}"),
            Style::default().fg(theme::ACCENT),
        ));
    }
    let model = vec![Span::styled(
        format!("  {model}"),
        Style::default().fg(theme::INTERACTION_STATUS_INFO),
    )];
    let mut labels = Vec::new();
    // A switch away from the assigned branch is a warning, not the branch
    // column's entry, so it is told in full among the labels rather than cut
    // to the column. Nested rows still show it beneath a worktree heading.
    if let Some(expected) = expected_branch {
        labels.push(Span::styled(
            format!(
                "  branch changed: {expected} → {}",
                branch.unwrap_or("detached head")
            ),
            Style::default()
                .fg(theme::UNCOMMITTED)
                .add_modifier(Modifier::BOLD),
        ));
    }
    labels.extend(state_labels(
        *status,
        stop_reason.as_deref(),
        *loading,
        rate_limited.as_deref(),
        *newly_idle,
        *completion,
    ));
    Some(InteractionCells {
        lead: main,
        name_index,
        tags: tag_cell,
        branch: branch_cell,
        model,
        labels,
    })
}

/// An interaction's last message as its row's second line has it, for any
/// view mentioning the interaction to quote it the same way.
pub(crate) fn last_message_text(text: &str) -> String {
    format!("« {text}")
}

/// The color an interaction's last message line is drawn in, before the
/// cursor's row lifts it or the others fade it.
pub(crate) const LAST_MESSAGE_COLOR: ratatui::style::Color = theme::SUBORDINATE_TEXT;

/// The marker a row's status is drawn with, on a faint block of its own hue,
/// so the state reads from the color down the left edge of the list and from
/// the shape up close. The cursor lights its row's block up, so the status
/// still reads under it.
pub(crate) fn status_block(status: InteractionStatus, selected: bool) -> Span<'static> {
    let (marker, color) = status_marker(status);
    let style = Style::default()
        .fg(color)
        .bg(status_background(status, selected))
        .add_modifier(Modifier::BOLD);
    // The running indicator is already as wide as the block.
    if matches!(status, InteractionStatus::Running { .. }) {
        Span::styled(marker, style)
    } else {
        Span::styled(format!(" {marker} "), style)
    }
}

/// The labels a row carries after its name and selection that say how it
/// came to be in its state, each in its own color.
pub(crate) fn state_labels(
    status: InteractionStatus,
    stop_reason: Option<&str>,
    loading: bool,
    rate_limited: Option<&str>,
    newly_idle: bool,
    completion: CompletionState,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if let Some(why) = stop_reason {
        // In the stop's own color, so how it ended is told apart down the
        // foot of the list without reading every word.
        let tone = match status {
            InteractionStatus::Stopped(tone) => tone.color(),
            _ => theme::INTERACTION_STATUS_INFO,
        };
        spans.push(Span::styled(format!("  {why}"), Style::default().fg(tone)));
    }
    if loading {
        spans.push(Span::styled(
            "  loading…",
            Style::default().fg(theme::INACTIVE),
        ));
    }
    if matches!(status, InteractionStatus::IdleFailed) {
        spans.push(Span::styled(
            "  TURN FAILED",
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(window) = rate_limited {
        spans.push(Span::styled(
            format!("  RATE LIMITED ({window})"),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if newly_idle {
        spans.push(Span::styled(
            "  NEWLY IDLE",
            Style::default()
                .fg(theme::SUCCESS)
                .add_modifier(Modifier::BOLD),
        ));
    }
    let badge = match completion {
        // Sealed by integration, which is the part worth reading.
        CompletionState::Sealed if status == InteractionStatus::Stopped(StopTone::Integrated) => {
            Some(("  INTEGRATED", theme::STOP_INTEGRATED))
        }
        CompletionState::Active => None,
        CompletionState::Completed => Some(("  COMPLETED", theme::STOP_COMPLETED)),
        CompletionState::Abandoned => Some(("  ABANDONED", theme::STOP_ABANDONED)),
        CompletionState::Sealed => Some(("  SEALED", theme::STOP_SEALED)),
    };
    if let Some((badge, color)) = badge {
        spans.push(Span::styled(
            badge,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    spans
}

/// The tree edge a branch hangs from, matching the Session picker's: a child
/// starts beneath its source's content, and each further level carries that
/// indent forward.
fn branch_indent(depth: usize) -> String {
    if depth == 0 {
        String::new()
    } else {
        format!("{}└─ ", "  ".repeat(depth))
    }
}

pub(crate) fn status_marker(status: InteractionStatus) -> (&'static str, ratatui::style::Color) {
    match status {
        // Each status has its own shape, so the list reads without its colors.
        InteractionStatus::Pending => ("●", theme::INFO),
        InteractionStatus::Running { events } => (running_indicator(events), theme::RUNNING),
        InteractionStatus::Idle => ("●", theme::SUCCESS),
        // Idle's shape, since it still takes messages, in the error's hue.
        InteractionStatus::IdleFailed => ("●", theme::ERROR),
        InteractionStatus::Background => ("◎", theme::MUTED_WARNING),
        // Not `#`, which starts the row's tags. In the hue of how it stopped,
        // as its reason is, so the reason reads from the left edge too.
        InteractionStatus::Stopped(StopTone::Integrated) => ("✓", theme::STOP_INTEGRATED),
        InteractionStatus::Stopped(tone) => ("■", tone.color()),
        // Not `!`, which marks work left uncommitted on the same row.
        InteractionStatus::Error => ("✗", theme::ERROR),
        InteractionStatus::Ended => ("–", theme::INACTIVE),
    }
}

/// The block behind a row's status marker: faint, or lit up under the cursor.
fn status_background(status: InteractionStatus, selected: bool) -> ratatui::style::Color {
    match (status, selected) {
        (InteractionStatus::Pending, false) => theme::PENDING_STATUS_BACKGROUND,
        (InteractionStatus::Pending, true) => theme::PENDING_STATUS_HIGHLIGHT,
        (InteractionStatus::Running { .. }, false) => theme::RUNNING_STATUS_BACKGROUND,
        (InteractionStatus::Running { .. }, true) => theme::RUNNING_STATUS_HIGHLIGHT,
        (InteractionStatus::Idle, false) => theme::IDLE_STATUS_BACKGROUND,
        (InteractionStatus::Idle, true) => theme::IDLE_STATUS_HIGHLIGHT,
        (InteractionStatus::Background, false) => theme::BACKGROUND_STATUS_BACKGROUND,
        (InteractionStatus::Background, true) => theme::BACKGROUND_STATUS_HIGHLIGHT,
        (InteractionStatus::Stopped(_), false) => theme::STOPPED_STATUS_BACKGROUND,
        (InteractionStatus::Stopped(_), true) => theme::STOPPED_STATUS_HIGHLIGHT,
        (InteractionStatus::Error | InteractionStatus::IdleFailed, false) => {
            theme::ERROR_STATUS_BACKGROUND
        }
        (InteractionStatus::Error | InteractionStatus::IdleFailed, true) => {
            theme::ERROR_STATUS_HIGHLIGHT
        }
        (InteractionStatus::Ended, false) => theme::ENDED_STATUS_BACKGROUND,
        (InteractionStatus::Ended, true) => theme::ENDED_STATUS_HIGHLIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    fn rendered(view: &InteractionNavigator<'_>) -> String {
        rendered_at(view, 80)
    }

    /// The screen in a pane `width` cells wide, for a row too long for the
    /// usual one.
    fn rendered_at(view: &InteractionNavigator<'_>, width: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, view, frame.area());
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    /// The rendered screen with each run of spaces cut to one, for checking
    /// what follows what on a row regardless of how its columns are padded.
    fn squeezed(screen: &str) -> String {
        screen
            .split(' ')
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// A lead column, padded out to the width it is held to in the test
    /// pane: half the 78 cells inside its border, short of
    /// [`LEAD_COLUMN_WIDTH`].
    fn lead(text: &str) -> String {
        format!("{text:<39}")
    }

    fn row(
        name: &'static str,
        depth: usize,
        last_message: Option<&'static str>,
    ) -> InteractionRow<'static> {
        InteractionRow::Interaction {
            name: name.into(),
            level: 0,
            branched_from: None,
            branches: 0,
            branch_link: BranchLink::None,
            depth,
            model: "gpt-5",
            branch: None,
            expected_branch: None,
            status: InteractionStatus::Stopped(StopTone::Paused),
            selected: false,
            loading: false,
            newly_idle: false,
            flashing: false,
            stop_reason: None,
            rate_limited: None,
            uncommitted: false,
            completion: CompletionState::Active,
            tags: &[],
            last_message,
        }
    }

    #[test]
    fn a_switched_branch_is_visible_under_a_worktree_heading() {
        for actual in ["other", "detached head"] {
            let mut switched = row("task", 0, None);
            if let InteractionRow::Interaction {
                level,
                branch,
                expected_branch,
                ..
            } = &mut switched
            {
                *level = 1;
                *branch = Some(actual);
                *expected_branch = Some("assigned");
            }
            let view = InteractionNavigator {
                rows: vec![switched],
                ..many_rows(0)
            };
            let screen = squeezed(&rendered(&view));
            assert!(
                screen.contains("task gpt-5 branch changed: assi"),
                "{screen}"
            );
        }
    }

    /// A branch hangs beneath its source with the Session picker's tree edge,
    /// and its last message is indented along with it.
    #[test]
    fn a_branch_is_drawn_nested_beneath_its_source() {
        let view = InteractionNavigator {
            rows: vec![row("source", 0, None), row("branch", 1, Some("forked"))],
            ..many_rows(0)
        };
        let screen = rendered(&view);
        assert!(
            screen.contains(&format!("┃{}  gpt-5", lead(" ■  source"))),
            "{screen}"
        );
        assert!(
            screen.contains(&format!("┃{}  gpt-5", lead("  └─  ■  branch"))),
            "{screen}"
        );
        assert!(screen.contains("┃         « forked"), "{screen}");
    }

    #[test]
    fn renders_grouping_activity_and_continuation_rows() {
        let tags = vec!["bug".into(), "urgent".into()];
        let view = InteractionNavigator {
            scope: "All".into(),
            all_workspaces: true,
            completion_filter: "completed hidden".into(),
            tag_filter: &[],
            filter: None,
            typing_filter: false,
            requested_offset: Some(0),
            rows: vec![
                InteractionRow::Workspace("Payments".into()),
                InteractionRow::Interaction {
                    name: "repair checkout".into(),
                    level: 0,
                    branched_from: None,
                    branches: 0,
                    branch_link: BranchLink::None,
                    depth: 0,
                    model: "gpt-5",
                    branch: Some("fix"),
                    expected_branch: None,
                    status: InteractionStatus::Running { events: 2 },
                    selected: true,
                    loading: false,
                    newly_idle: true,
                    flashing: false,
                    stop_reason: None,
                    rate_limited: None,
                    uncommitted: false,
                    completion: CompletionState::Active,
                    tags: &tags,
                    last_message: Some("The checks are green."),
                },
            ],
        };
        let screen = rendered_at(&view, 120);
        assert!(screen.contains("Payments"), "{screen}");
        assert!(
            squeezed(&screen).contains("repair checkout fix #bug #urgent gpt-5 NEWLY IDLE"),
            "{screen}"
        );
        assert!(screen.contains("  NEWLY IDLE"), "{screen}");
        assert!(screen.contains("The checks are green."), "{screen}");
        assert_eq!(height(&view, 12), 5);
    }

    fn placed(
        name: &'static str,
        level: usize,
        depth: usize,
        branched_from: Option<&'static str>,
    ) -> InteractionRow<'static> {
        InteractionRow::Interaction {
            name: name.into(),
            level,
            depth,
            branched_from: branched_from.map(Cow::Borrowed),
            branches: 0,
            branch_link: BranchLink::None,
            model: "sonnet",
            branch: Some("feature"),
            expected_branch: None,
            status: InteractionStatus::Idle,
            selected: false,
            loading: false,
            newly_idle: false,
            flashing: false,
            stop_reason: None,
            rate_limited: None,
            uncommitted: false,
            completion: CompletionState::Active,
            tags: &[],
            last_message: (level > 0).then_some("preview"),
        }
    }

    fn directory(
        name: &'static str,
        level: usize,
        into: Option<&'static str>,
    ) -> InteractionRow<'static> {
        InteractionRow::Directory {
            name: name.into(),
            level,
            into: into.map(Cow::Borrowed),
        }
    }

    fn lines(view: &InteractionNavigator<'_>) -> Vec<String> {
        rendered(view)
            .chars()
            .collect::<Vec<_>>()
            .chunks(80)
            .map(|line| line.iter().collect::<String>())
            .collect()
    }

    /// Tree lines show workspace and directory membership without color;
    /// grouped rows also drop the branch the directory heading already gives.
    #[test]
    fn rows_have_tree_lines_under_their_headings() {
        let row = |name: &'static str, grouped: bool| placed(name, grouped.into(), 0, None);
        let all = InteractionNavigator {
            all_workspaces: true,
            rows: vec![
                InteractionRow::Workspace("Payments".into()),
                row("alone", false),
                directory("checkout", 0, None),
                row("first", true),
                row("second", true),
                directory("other", 0, None),
                row("third", true),
            ],
            ..many_rows(0)
        };
        let screen = lines(&all);
        assert!(screen[1].starts_with("┃Payments ───"), "{screen:#?}");
        assert!(screen[1].ends_with("─┃"), "{screen:#?}");
        assert!(
            screen[2].starts_with(&format!("┃{}  feature  sonnet", lead("  ●  alone"))),
            "{screen:#?}"
        );
        assert!(screen[3].starts_with("┃ [checkout]"), "{screen:#?}");
        assert!(
            screen[4].starts_with(&format!("┃{}{:9}  sonnet ", lead(" ├─  ●  first"), "")),
            "{screen:#?}"
        );
        assert!(
            screen[6].starts_with(&format!("┃{}{:9}  sonnet ", lead(" └─  ●  second"), "")),
            "{screen:#?}"
        );

        assert!(screen[5].starts_with("┃ │      « preview"), "{screen:#?}");
        assert!(screen[7].starts_with("┃        « preview"), "{screen:#?}");
        assert!(screen[8].starts_with("┃ [other]"), "{screen:#?}");
        assert!(
            screen[9].starts_with(&format!("┃{}{:9}  sonnet", lead(" └─  ●  third"), "")),
            "{screen:#?}"
        );

        // In Workspace scope there is no Workspace heading to step in under.
        let one = InteractionNavigator {
            rows: vec![
                row("alone", false),
                directory("checkout", 0, None),
                row("first", true),
                row("outside", false),
            ],
            ..many_rows(0)
        };
        let screen = lines(&one);
        assert!(
            screen[1].starts_with(&format!("┃{}  feature  sonnet", lead(" ●  alone"))),
            "{screen:#?}"
        );
        assert!(screen[2].starts_with("┃[checkout]"), "{screen:#?}");
        assert!(
            screen[3].starts_with(&format!("┃{}{:9}  sonnet ", lead("└─  ●  first"), "")),
            "{screen:#?}"
        );
        assert!(screen[4].starts_with("┃       « preview"), "{screen:#?}");
        assert!(
            screen[5].starts_with(&format!("┃{}  feature  sonnet", lead(" ●  outside"))),
            "{screen:#?}"
        );
    }

    /// A worktree integrated into another is headed beneath that one's
    /// heading, after its rows, with the line of the outer worktree carried
    /// past it. A branch in its source's worktree hangs from the source's row;
    /// one in another worktree names its source, which counts its branches.
    #[test]
    fn nested_worktrees_and_branches_are_drawn_in_their_own_ways() {
        let mut rows = vec![
            directory("auth-rework", 0, Some("main")),
            placed("rework", 1, 0, None),
            placed("try jwt", 1, 1, None),
            placed("review", 1, 0, None),
            directory("token-tests", 1, Some("styra/auth-rework")),
            placed("tests", 2, 0, None),
            directory("jwt-spike", 1, Some("styra/auth-rework")),
            placed("spike", 2, 0, Some("rework")),
        ];
        for row in &mut rows {
            if let InteractionRow::Interaction {
                name,
                branches,
                last_message,
                ..
            } = row
            {
                *last_message = None;
                if name == "rework" {
                    *branches = 1;
                }
            }
        }
        let view = InteractionNavigator {
            rows,
            ..many_rows(0)
        };
        let screen = lines(&view);
        // A fork icon starts the name it marks; the source a branch came from
        // follows the name. With no branch on any row, a heading's arrow
        // starts where the model does.
        let row = |lead_text: &str, name: &str| {
            format!("┃{}  sonnet", lead(&format!("{lead_text}  {name}")))
        };
        let expected = [
            format!("┃{:38} → main", "[auth-rework]"),
            row("├─  ●", "↱1 rework"),
            row("│    └─  ●", "try jwt"),
            row("├─  ●", "review"),
            format!("┃{:38} → styra/auth-rework", "├─ [token-tests]"),
            row("│  └─  ●", "tests"),
            format!("┃{:38} → styra/auth-rework", "└─ [jwt-spike]"),
            row("   └─  ●", "↳ spike from rework"),
        ];
        for (line, expected) in screen[1..].iter().zip(&expected) {
            assert!(line.starts_with(expected.as_str()), "{screen:#?}");
        }
    }

    /// A branch drawn apart from its source is tied to the cursor from
    /// either end: the other end's name is underlined, and the fork icons at
    /// both ends are lit, so a source reads apart from a branch.
    #[test]
    fn a_branch_is_marked_from_either_end_of_it() {
        // `rework` on line 1 counts `spike`, on line 2, as its branch.
        let drawn = |cursor_on_source: bool| {
            let link = |source: bool| match (source, cursor_on_source) {
                (true, false) => BranchLink::Source,
                (false, true) => BranchLink::Branch,
                _ => BranchLink::None,
            };
            let mut rows = vec![
                placed("rework", 0, 0, None),
                placed("spike", 0, 0, Some("rework")),
            ];
            for (index, row) in rows.iter_mut().enumerate() {
                if let InteractionRow::Interaction {
                    branches,
                    branch_link,
                    selected,
                    last_message,
                    ..
                } = row
                {
                    let source = index == 0;
                    *branches = source.into();
                    *branch_link = link(source);
                    *selected = source == cursor_on_source;
                    *last_message = None;
                }
            }
            let view = InteractionNavigator {
                rows,
                ..many_rows(0)
            };
            let mut terminal = Terminal::new(TestBackend::new(80, 6)).unwrap();
            terminal
                .draw(|frame| {
                    render(frame, &view, frame.area());
                })
                .unwrap();
            terminal.backend().buffer().clone()
        };
        let cell = |buffer: &Buffer, symbol: &str, y: u16| {
            let x = (0..80)
                .find(|&x| buffer[(x, y)].symbol() == symbol)
                .expect("line draws the symbol");
            buffer[(x, y)].clone()
        };
        let lit = |selected: bool| {
            let style = Style::default().fg(theme::BRANCH_LINK);
            if selected {
                style
            } else {
                faded_style(style, UNSELECTED_ROW_FADE)
            }
            .fg
            .unwrap()
        };
        for cursor_on_source in [true, false] {
            let buffer = drawn(cursor_on_source);
            assert_eq!(cell(&buffer, "↱", 1).fg, lit(cursor_on_source));
            assert_eq!(cell(&buffer, "↳", 2).fg, lit(!cursor_on_source));
            let (other, y) = if cursor_on_source { ("s", 2) } else { ("r", 1) };
            assert!(cell(&buffer, other, y)
                .modifier
                .contains(Modifier::UNDERLINED));
        }
    }

    /// An unheaded worktree's long agent branch — the target a heading names
    /// after `→` — is cut to the branch column, so it moves no column after
    /// it; a row with no branch of its own keeps the column empty.
    #[test]
    fn a_long_branch_is_cut_to_its_column() {
        let mut long = placed("solo", 0, 0, None);
        if let InteractionRow::Interaction { branch, .. } = &mut long {
            *branch = Some("styra/align-live-interactions-columns-1792571015193");
        }
        let view = InteractionNavigator {
            rows: vec![
                placed("alone", 0, 0, None),
                long,
                directory("checkout", 0, Some("styra/align-live-interactions-columns")),
                placed("first", 1, 0, None),
            ],
            ..many_rows(0)
        };
        let screen = lines(&view);
        // The narrow test pane leaves the branch 17 cells after the model
        // and room for a label.
        assert!(
            screen[1].starts_with(&format!("┃{}{:17}  sonnet", lead(" ●  alone"), "  feature")),
            "{screen:#?}"
        );
        assert!(
            screen[2].starts_with(&format!("┃{}  styra/align-li…  sonnet", lead(" ●  solo"))),
            "{screen:#?}"
        );
        // The heading's merge target lines up with the rows' branches.
        assert!(
            screen[3].starts_with(&format!("┃{:38} → styra/align-live", "[checkout]")),
            "{screen:#?}"
        );
        assert!(
            screen[4].starts_with(&format!("┃{}{:17}  sonnet", lead("└─  ●  first"), "")),
            "{screen:#?}"
        );
    }

    /// The first column is a set width: a long name is cut short there, and
    /// the branch and tags line up after it whatever the names.
    #[test]
    fn a_long_name_is_cut_at_the_first_column_and_tags_line_up() {
        let bug = vec!["bug".to_owned()];
        let mut long = placed(
            "a prompt far too long to fit in the first column of the list",
            0,
            0,
            None,
        );
        let mut short = placed("short", 0, 0, None);
        for row in [&mut long, &mut short] {
            if let InteractionRow::Interaction { tags, .. } = row {
                *tags = &bug;
            }
        }
        let view = InteractionNavigator {
            rows: vec![long, short],
            ..many_rows(0)
        };
        // Wide enough that the branch and tags after the name are not cut.
        let screen = rendered_at(&view, 120)
            .chars()
            .collect::<Vec<_>>()
            .chunks(120)
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>();
        assert!(
            screen[1]
                .starts_with("┃ ●  a prompt far too long to fit in the…  feature  #bug  sonnet"),
            "{screen:#?}"
        );
        assert!(
            screen[2].starts_with(&format!("┃{:40}  feature  #bug  sonnet", " ●  short")),
            "{screen:#?}"
        );
    }

    /// Where a cell's column, held to a share of the pane, is narrower than
    /// the cell, the cell is cut short there: the columns after it stay where
    /// every other row has them.
    #[test]
    fn a_cell_wider_than_its_column_is_cut_and_moves_nothing() {
        let many = ["refactor", "performance", "urgent", "frontend", "backend"]
            .map(String::from)
            .to_vec();
        let mut wide = placed("wide", 0, 0, None);
        let mut narrow = placed("narrow", 0, 0, None);
        if let InteractionRow::Interaction {
            tags,
            model,
            stop_reason,
            ..
        } = &mut wide
        {
            *tags = &many;
            *model = "gpt-5.1-codex-max-preview";
            *stop_reason = Some("paused".into());
        }
        if let InteractionRow::Interaction { stop_reason, .. } = &mut narrow {
            *stop_reason = Some("paused".into());
        }
        let view = InteractionNavigator {
            rows: vec![wide, narrow],
            ..many_rows(0)
        };
        for width in [80u16, 120] {
            let screen = rendered_at(&view, width)
                .chars()
                .collect::<Vec<_>>()
                .chunks(width.into())
                .map(|line| line.iter().collect::<String>())
                .collect::<Vec<_>>();
            let at = |line: &str, needle: &str| {
                line.find(needle)
                    .map(|byte| line[..byte].chars().count())
                    .unwrap_or_else(|| panic!("no {needle:?} in {screen:#?}"))
            };
            assert!(screen[1].contains('…'), "{screen:#?}");
            for (wide, narrow) in [("  gpt", "  sonnet"), ("  paused", "  paused")] {
                assert_eq!(at(&screen[1], wide), at(&screen[2], narrow), "{screen:#?}");
            }
        }
    }

    /// One row's uncommitted flag keeps room on every row, so the names
    /// still start together; a prompt's line breaks read as spaces.
    #[test]
    fn names_line_up_beside_an_uncommitted_flag() {
        let mut flagged = placed("flagged", 0, 0, None);
        if let InteractionRow::Interaction { uncommitted, .. } = &mut flagged {
            *uncommitted = true;
        }
        let view = InteractionNavigator {
            rows: vec![flagged, placed("two\nlines", 0, 0, None)],
            ..many_rows(0)
        };
        let screen = lines(&view);
        assert!(screen[1].starts_with("┃ ●  ! flagged "), "{screen:#?}");
        assert!(screen[2].starts_with("┃ ●    two lines "), "{screen:#?}");
    }

    /// Rows explain whether they stopped, failed a turn, or are waiting on
    /// a plan window, so the operator can decide what to do next.
    #[test]
    fn interaction_state_labels_explain_what_is_holding_up_work() {
        for (status, stop_reason, rate_limited, width, label) in [
            (
                InteractionStatus::Stopped(StopTone::Paused),
                Some("paused"),
                None,
                80,
                "paused",
            ),
            (InteractionStatus::IdleFailed, None, None, 80, "TURN FAILED"),
            (
                InteractionStatus::Idle,
                None,
                Some("five_hour"),
                120,
                "RATE LIMITED (five_hour)",
            ),
        ] {
            let mut row = placed("repair checkout", 0, 0, None);
            if let InteractionRow::Interaction {
                branch,
                status: row_status,
                stop_reason: row_stop_reason,
                rate_limited: row_rate_limited,
                ..
            } = &mut row
            {
                *branch = None;
                *row_status = status;
                *row_stop_reason = stop_reason.map(Into::into);
                *row_rate_limited = rate_limited.map(Into::into);
            }
            let view = InteractionNavigator {
                rows: vec![row],
                ..many_rows(0)
            };
            let screen = squeezed(&rendered_at(&view, width));
            assert!(
                screen.contains(&format!("repair checkout sonnet {label}")),
                "{label}: {screen}"
            );
        }
        assert_eq!(status_marker(InteractionStatus::IdleFailed).1, theme::ERROR);
    }

    fn many_rows(selected: usize) -> InteractionNavigator<'static> {
        InteractionNavigator {
            scope: "Payments".into(),
            all_workspaces: false,
            completion_filter: "completed hidden".into(),
            tag_filter: &[],
            filter: None,
            typing_filter: false,
            requested_offset: Some(0),
            rows: (0..8)
                .map(|index| InteractionRow::Interaction {
                    name: format!("task {index}").into(),
                    level: 0,
                    branched_from: None,
                    branches: 0,
                    branch_link: BranchLink::None,
                    depth: 0,
                    model: "sonnet",
                    branch: None,
                    expected_branch: None,
                    status: InteractionStatus::Idle,
                    selected: index == selected,
                    loading: false,
                    newly_idle: false,
                    flashing: false,
                    stop_reason: None,
                    rate_limited: None,
                    uncommitted: false,
                    completion: CompletionState::Active,
                    tags: &[],
                    last_message: Some("done"),
                })
                .collect(),
        }
    }

    /// A row whose message line does not fit is drawn as far as it goes, so
    /// the pane is filled to its foot, and the border says how much is left.
    #[test]
    fn a_list_longer_than_the_pane_fills_it_and_says_how_much_is_below() {
        let screen = rendered(&many_rows(0));
        let lines = screen
            .chars()
            .collect::<Vec<_>>()
            .chunks(80)
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>();
        // Ten lines inside the border: five rows of two lines each.
        assert!(lines[9].contains("task 4"), "{screen}");
        assert!(lines[10].contains("done"), "{screen}");
        assert!(lines[11].contains("6 more lines"), "{screen}");
        assert!(!lines[0].contains("more line"), "{screen}");
    }

    #[test]
    fn the_list_scrolls_to_keep_the_cursor_in_view() {
        let screen = rendered(&many_rows(7));
        let lines = screen
            .chars()
            .collect::<Vec<_>>()
            .chunks(80)
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>();
        assert!(lines[9].contains("task 7"), "{screen}");
        assert!(lines[10].contains("done"), "{screen}");
        assert!(!screen.contains("task 2 "), "{screen}");
        assert!(lines[0].contains("6 more lines"), "{screen}");
        assert!(!lines[11].contains("more line"), "{screen}");
    }

    #[test]
    fn a_fresh_list_starts_at_the_bottom_with_the_current_interaction_selected() {
        let mut view = many_rows(4);
        view.requested_offset = None;
        assert_eq!(scroll_offset(&view, Some(4), 14, 16), 2);
        // A saved offset still wins when the selection fits within its margins.
        view.requested_offset = Some(0);
        assert_eq!(scroll_offset(&view, Some(4), 14, 16), 0);
        // An older current interaction must remain visible on the first draw.
        view = many_rows(0);
        view.requested_offset = None;
        assert_eq!(scroll_offset(&view, Some(0), 10, 16), 0);
    }

    #[test]
    fn an_unselected_list_shows_the_bottom() {
        // An index outside the list leaves every interaction unselected.
        assert_eq!(drawn_offset(usize::MAX, 0), 6);
        assert_eq!(drawn_offset(usize::MAX, 2), 6);
        let screen = rendered(&many_rows(usize::MAX));
        assert!(screen.contains("task 7"), "{screen}");
        assert!(!screen.contains("task 0"), "{screen}");
    }

    #[test]
    fn an_unselected_list_that_fits_starts_at_the_top() {
        let mut view = many_rows(usize::MAX);
        view.rows.truncate(2);
        assert_eq!(scroll_offset(&view, None, 10, 4), 0);
        view.rows.clear();
        assert_eq!(scroll_offset(&view, None, 10, 0), 0);
    }

    /// The line [`render`] draws [`many_rows`] from, in a pane ten lines tall
    /// inside its border: five two-line rows.
    fn drawn_offset(selected: usize, requested_offset: usize) -> usize {
        let view = InteractionNavigator {
            requested_offset: Some(requested_offset),
            ..many_rows(selected)
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        let mut offset = None;
        terminal
            .draw(|frame| offset = Some(render(frame, &view, frame.area())))
            .unwrap();
        offset.unwrap()
    }

    #[test]
    fn scrolling_keeps_two_rows_of_margin_until_the_list_ends() {
        for (rule, selected, requested, expected) in [
            ("free movement at top", 0, 0, 0),
            ("free movement at top", 1, 0, 0),
            ("free movement at top", 2, 0, 0),
            ("free movement between margins", 4, 4, 4),
            ("approaching lower edge", 3, 0, 2),
            ("inside bottom margin", 5, 6, 6),
            ("approaching upper edge", 4, 6, 4),
            ("stop at first row", 0, 6, 0),
            ("stop at last row", 7, 0, 6),
            ("clamp past end", 7, 100, 6),
        ] {
            assert_eq!(
                drawn_offset(selected, requested),
                expected,
                "{rule}: selected={selected}, requested={requested}"
            );
        }
    }

    /// Every row but the cursor's is gently faded, with supporting text
    /// further subdued so the cursor's row stays the brightest in the list.
    #[test]
    fn the_cursor_row_is_lifted_and_the_rest_faded() {
        let view = many_rows(0);
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &view, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        // The first cell drawing `symbol` on line `y`: `t` starts each name,
        // `o` the model, `d` the message.
        let color = |symbol: &str, y: u16| {
            let x = (0..80)
                .find(|&x| buffer[(x, y)].symbol() == symbol)
                .expect("line draws the symbol");
            buffer[(x, y)].fg
        };
        let faded = |color| {
            faded_style(Style::default().fg(color), UNSELECTED_FADE)
                .fg
                .unwrap()
        };
        let brightened = |color| {
            blended_style(Style::default().fg(color), theme::TEXT, SELECTED_BRIGHTEN)
                .fg
                .unwrap()
        };
        let unselected = |color| {
            faded_style(Style::default().fg(color), UNSELECTED_ROW_FADE)
                .fg
                .unwrap()
        };
        // The cursor's row on lines 1 and 2, the next row on lines 3 and 4.
        assert_eq!(color("●", 1), theme::SUCCESS);
        assert_eq!(color("t", 1), theme::SELECTED_LIVE_INTERACTION_TEXT);
        assert_eq!(color("o", 1), brightened(theme::INTERACTION_STATUS_INFO));
        assert_eq!(color("d", 2), brightened(theme::SUBORDINATE_TEXT));
        assert_eq!(color("●", 3), unselected(theme::SUCCESS));
        assert_eq!(color("t", 3), unselected(theme::TEXT));
        assert_eq!(
            color("o", 3),
            unselected(faded(theme::INTERACTION_STATUS_INFO))
        );
        assert_eq!(color("d", 4), unselected(faded(theme::SUBORDINATE_TEXT)));
    }

    /// The cursor's row, message line and all, is marked by a bar over the
    /// left border in the selection color; the dropdown has a thick frame.
    #[test]
    fn the_cursor_row_is_marked_on_the_left_border() {
        let lines = rendered(&many_rows(1))
            .chars()
            .collect::<Vec<_>>()
            .chunks(80)
            .map(|line| line.iter().collect::<String>())
            .collect::<Vec<_>>();
        assert!(
            lines[3].starts_with("┃") && lines[3].contains("task 1"),
            "{lines:#?}"
        );
        assert!(
            lines[4].starts_with("┃") && lines[4].contains("done"),
            "{lines:#?}"
        );
        for line in [1, 2, 5, 6] {
            assert!(lines[line].starts_with("┃"), "{lines:#?}");
        }
    }

    /// Each way of being finished with a Session has its own badge, so one
    /// given up on never reads as one that got done.
    #[test]
    fn each_completion_state_has_its_own_badge() {
        for (completion, badge) in [
            (CompletionState::Completed, "  COMPLETED"),
            (CompletionState::Abandoned, "  ABANDONED"),
            (CompletionState::Sealed, "  SEALED"),
        ] {
            let mut view = many_rows(0);
            view.rows.truncate(1);
            if let InteractionRow::Interaction {
                completion: row, ..
            } = &mut view.rows[0]
            {
                *row = completion;
            }
            let screen = rendered(&view);
            assert!(screen.contains(badge), "{completion:?}: {screen}");
            for (other, other_badge) in [
                (CompletionState::Completed, "  COMPLETED"),
                (CompletionState::Abandoned, "  ABANDONED"),
                (CompletionState::Sealed, "  SEALED"),
            ] {
                if other != completion {
                    assert!(!screen.contains(other_badge), "{completion:?}: {screen}");
                }
            }
        }
        let mut view = many_rows(0);
        view.rows.truncate(1);
        let screen = rendered(&view);
        assert!(!screen.contains("COMPLETED"), "{screen}");
        assert!(!screen.contains("ABANDONED"), "{screen}");
        assert!(!screen.contains("SEALED"), "{screen}");
    }

    /// A Session sealed by integration says so, and is marked as done rather
    /// than as merely stopped.
    #[test]
    fn a_session_sealed_by_integration_reads_as_integrated() {
        let mut view = many_rows(0);
        view.rows.truncate(1);
        if let InteractionRow::Interaction {
            completion, status, ..
        } = &mut view.rows[0]
        {
            *completion = CompletionState::Sealed;
            *status = InteractionStatus::Stopped(StopTone::Integrated);
        }
        let screen = rendered(&view);
        assert!(screen.contains("✓"), "{screen}");
        assert!(screen.contains("  INTEGRATED"), "{screen}");
        assert!(!screen.contains("SEALED"), "{screen}");
    }
}
