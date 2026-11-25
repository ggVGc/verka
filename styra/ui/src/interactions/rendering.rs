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

/// How far, in tenths, interactions other than the cursor's fade toward the
/// background, including nested names, status markers and message lines.
const UNSELECTED_ROW_FADE: u16 = 2;

/// Additional fading for supporting text on rows other than the cursor's.
const UNSELECTED_FADE: u16 = 2;

/// How far, in tenths, the cursor's row has the text after its name lifted
/// toward [`theme::TEXT`].
const SELECTED_BRIGHTEN: u16 = 4;

/// The dropdown always takes half the available height, so it does not jump in
/// size as filters, scope or new interactions change how many rows it has.
pub fn height(available: u16) -> u16 {
    available.saturating_div(2).max(3).min(available)
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
        // Scope, completion, time and tag filters are state the list cannot be
        // read without; the commands that change them are behind `?`.
        .title(format!(
            " {} · interactions · {} · {}{} · ? keys ",
            view.scope,
            view.completion_filter,
            view.time_filter,
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
    let widths = column_widths(view, inner.width.into());
    let selected = view
        .rows
        .iter()
        .position(|row| matches!(row, InteractionRow::Interaction { selected: true, .. }));
    // Top-level interactions have a faint background; branches stay darker.
    // The background also covers each interaction's message line.
    // The cursor's row is drawn on a slightly lighter tint in its place.
    let mut items = view
        .rows
        .iter()
        .map(|row| {
            let mut item = row_item(row, view.all_workspaces, inner.width.into(), &widths);
            match row {
                InteractionRow::Workspace(_) => {}
                InteractionRow::Interaction {
                    selected,
                    flashing,
                    depth,
                    ..
                } => {
                    let background = if *flashing {
                        Some(theme::FLASHED_ROW_BACKGROUND)
                    } else if *selected {
                        Some(theme::SELECTED_ROW_BACKGROUND)
                    } else {
                        (*depth == 0).then_some(theme::ALTERNATE_ROW_BACKGROUND)
                    };
                    if let Some(background) = background {
                        for line in &mut item {
                            line.style = line.style.bg(background);
                        }
                    }
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
    if let (Some(selected), true) = (selected, inner.x > area.x) {
        let start = view.rows[..selected].iter().map(row_height).sum::<usize>();
        let end = start + row_height(&view.rows[selected]);
        let (start, end) = (start.max(offset), end.min(offset + viewport));
        if start < end {
            crate::chrome::selected_row_edge(
                frame,
                area.x,
                inner.y + (start - offset) as u16..inner.y + (end - offset) as u16,
            );
        }
    }
    offset
}

/// The first line to draw. The view stays where it was while the cursor moves
/// inside it, and scrolls only as far as it takes to keep [`SCROLL_MARGIN`]
/// rows showing past the cursor, fewer where the pane is too short for them.
/// A view scrolled on its own stays where it was put, cursor or not.
fn scroll_offset(
    view: &InteractionNavigator<'_>,
    selected: Option<usize>,
    viewport: usize,
    total: usize,
) -> usize {
    // The foot of the pane is never left empty while there is list to fill it.
    let last = total.saturating_sub(viewport);
    let mut offset = view.requested_offset.unwrap_or(0).min(last);
    let Some(selected) = selected.filter(|_| !view.scrolled_away) else {
        return offset;
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

/// The initial width of the first column: tree lines, status block and name.
/// Longer names can use space left after the other columns are allocated.
const LEAD_COLUMN_WIDTH: usize = 40;

/// How wide each aligned column of the interaction rows is drawn, so the
/// tags, the branch, the state labels, and the model each
/// start at the same place down the list. The first column starts at
/// [`LEAD_COLUMN_WIDTH`] and grows into unused space for longer names. The
/// branch and tags columns fit their widest entries, though the branch gives
/// way to the first column and the state labels in a narrow pane; the others
/// grow no wider than their share of the remaining pane. Every cell is held
/// to its column's width — see [`fit_cell`] — so a longer entry is cut short
/// rather than moving the rest of its row. A branch is cut in the middle, keeping both its start and its
/// end in view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ColumnWidths {
    /// Whether any row is flagged as leaving work uncommitted, so every row
    /// keeps room for the flag and the names start together.
    uncommitted: bool,
    lead: usize,
    tags: usize,
    branch: usize,
    model: usize,
    labels: usize,
}

/// See [`ColumnWidths`]. `width` is the pane's inner width.
fn column_widths(view: &InteractionNavigator<'_>, width: usize) -> ColumnWidths {
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
    for row in &view.rows {
        if let Some(cells) = interaction_cells(row, view.all_workspaces, &widths) {
            widths.lead = widths.lead.max(spans_width(&cells.lead));
            widths.tags = widths.tags.max(spans_width(&cells.tags));
            widths.branch = widths.branch.max(spans_width(&cells.branch));
            widths.model = widths.model.max(spans_width(&cells.model));
            widths.labels = widths.labels.max(spans_width(&cells.labels));
        }
    }
    // Keep tags whole. The branch gets what is left once the lead has its
    // initial width, or as much of it as the names need, and the state
    // labels their room, so a long branch is cut before the names are.
    // Fit the model and state labels in the remaining room.
    let reserved = LEAD_COLUMN_WIDTH.min(widths.lead) + LABEL_ROOM.min(widths.labels);
    let branch = widths
        .branch
        .min(width.saturating_sub(widths.tags + reserved));
    let tags = widths.tags;
    let lead = LEAD_COLUMN_WIDTH
        .min(width / 2)
        .min(width.saturating_sub(branch + tags));
    let room = width.saturating_sub(lead + branch + tags + LABEL_ROOM);
    let model = widths.model.min(width / 4).min(room);
    let labels = widths
        .labels
        .min(width.saturating_sub(lead + branch + tags + model));
    let lead = lead.max(
        widths
            .lead
            .min(width.saturating_sub(branch + tags + model + labels)),
    );
    ColumnWidths {
        uncommitted: widths.uncommitted,
        lead,
        tags,
        branch,
        model,
        labels,
    }
}

/// How much room the other columns reserve for the state labels:
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
    fit_cell_with(spans, start, width, |line, width| {
        crate::text::truncate(line, width, None)
    });
}

/// [`fit_cell`], cutting a wider cell short with `cut`.
fn fit_cell_with(
    spans: &mut Vec<Span<'static>>,
    start: usize,
    width: usize,
    cut: fn(Line<'static>, usize) -> Line<'static>,
) {
    let cell = spans.split_off(start);
    let cell = cut(Line::from(cell), width);
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
/// headings, which every row then steps in beneath. `width` is
/// the pane's inner width, which a Workspace heading's rule runs out to.
/// `widths` lines the interaction rows' columns up.
fn row_item(
    row: &InteractionRow<'_>,
    under_workspaces: bool,
    width: usize,
    widths: &ColumnWidths,
) -> Vec<Line<'static>> {
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
    let Some(cells) = interaction_cells(row, under_workspaces, widths) else {
        unreachable!()
    };
    let InteractionCells {
        lead: mut main,
        name_index,
        tags,
        branch,
        model,
        labels,
    } = cells;
    // The name ends the lead, so it is what a long lead loses.
    fit_cell(&mut main, 0, widths.lead);
    let start = main.len();
    main.extend(tags);
    fit_cell(&mut main, start, widths.tags);
    // A branch's start and end tell branches apart more often than its
    // middle does, so it loses its middle.
    let start = main.len();
    main.extend(branch);
    fit_cell_with(
        &mut main,
        start,
        widths.branch,
        crate::text::truncate_middle,
    );
    for (cell, width) in [(labels, widths.labels), (model, widths.model)] {
        let start = main.len();
        main.extend(cell);
        fit_cell(&mut main, start, width);
    }
    // No trailing spaces where nothing follows to be lined up.
    while main.len() > name_index + 1
        && main
            .last()
            .is_some_and(|span| span.content.trim().is_empty())
    {
        main.pop();
    }
    // Everything after the name is lifted toward white and bolded on the
    // cursor's row, and faded on the others.
    // The edges, status marker and name keep their base styles here.
    // The row fade below keeps top-level names bright.
    let message_style = Style::default().fg(LAST_MESSAGE_COLOR);
    let adjust = |style: Style| {
        if *selected {
            blended_style(style, theme::TEXT, SELECTED_BRIGHTEN).add_modifier(Modifier::BOLD)
        } else {
            faded_style(style, UNSELECTED_FADE)
        }
    };
    for span in main.iter_mut().skip(name_index + 1) {
        if span.style != shared_worktree_style() {
            span.style = adjust(span.style);
        }
    }
    let message_style = adjust(message_style);
    let edge = branch_indent(*depth);
    let mut lines = vec![Line::from(main)];
    if let Some(text) = last_message {
        lines.push(Line::from(vec![
            Span::raw(workspace_edge),
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
    if !*selected {
        for (line_index, line) in lines.iter_mut().enumerate() {
            line.style = faded_style(line.style, UNSELECTED_ROW_FADE);
            for (span_index, span) in line.spans.iter_mut().enumerate() {
                let top_level_name = *depth == 0 && line_index == 0 && span_index == name_index;
                if !top_level_name && span.style != shared_worktree_style() {
                    span.style = faded_style(span.style, UNSELECTED_ROW_FADE);
                }
            }
        }
    }
    lines
}

/// The columns of an interaction row's first line — `None` for a heading.
/// See [`row_item`] for the arguments.
fn interaction_cells(
    row: &InteractionRow<'_>,
    under_workspaces: bool,
    widths: &ColumnWidths,
) -> Option<InteractionCells> {
    let InteractionRow::Interaction {
        name,
        depth,
        branched_from,
        branches,
        branch_link,
        model,
        branch,
        expected_branch,
        worktree,
        same_as_source,
        shared_worktree,
        into,
        status,
        selected,
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
    let workspace_edge = if under_workspaces { " " } else { "" };
    let edge = branch_indent(*depth);
    let mut main = vec![
        Span::raw(workspace_edge),
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
    } else if *depth == 0 {
        Style::default().fg(theme::TOP_LEVEL_INTERACTION_NAME)
    } else {
        Style::default().fg(theme::NESTED_INTERACTION_NAME)
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
    // Set off from the name before them like every other column.
    let mut tag_cell = Vec::new();
    if !tags.is_empty() {
        tag_cell.push(Span::styled(
            format!("  #{}", tags.join(" #")),
            Style::default().fg(theme::INTERACTION_TAG),
        ));
    }
    let mut branch_cell = vec![Span::raw("  ")];
    // A switch away from the assigned branch is a warning in the branch
    // column, naming the branch checked out and the one assigned.
    let branch_span = if let Some(expected) = expected_branch {
        Some(Span::styled(
            format!("{} (≠ {expected})", branch.unwrap_or("detached head")),
            Style::default()
                .fg(theme::UNCOMMITTED)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        branch
            // A worktree named after its branch already says it, as Styra's
            // own do: their branch is the worktree's name under `styra/`.
            .filter(|branch| {
                let last = branch.rsplit('/').next().unwrap_or(branch);
                worktree.as_deref() != Some(last)
            })
            .map(|branch| Span::styled(branch.to_owned(), Style::default().fg(theme::ACCENT)))
    };
    // Where a row shares the cursor's worktree, its worktree's name is lifted
    // as on the cursor's own row, rather than faded with the rest of it.
    let worktree_style = if *shared_worktree {
        shared_worktree_style()
    } else {
        Style::default().fg(theme::DIRECTORY_NAME)
    };
    // A linked worktree is named ahead of its branch.
    if let Some(worktree) = worktree {
        branch_cell.push(Span::styled(format!("[{worktree}]"), worktree_style));
        if branch_span.is_some() {
            branch_cell.push(Span::raw(" "));
        }
    }
    branch_cell.extend(branch_span);
    // Where its work goes.
    if let Some(into) = into.as_ref().filter(|_| worktree.is_some()) {
        branch_cell.push(Span::styled(
            format!(" → {into}"),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    // A branch on its source's worktree and branch would only repeat what
    // the row above says, so it points up at it instead. A switch away from
    // the assigned branch is still spelled out.
    if *same_as_source && expected_branch.is_none() && branch_cell.len() > 1 {
        branch_cell.truncate(1);
        let up_style = if *shared_worktree {
            worktree_style
        } else {
            Style::default().fg(theme::MUTED_TEXT)
        };
        branch_cell.push(Span::styled("↑", up_style));
    }
    if branch_cell.len() == 1 {
        branch_cell.clear();
    }
    let model = vec![Span::styled(
        format!("  {model}"),
        Style::default().fg(theme::INTERACTION_STATUS_INFO),
    )];
    let labels = state_labels(
        *status,
        stop_reason.as_deref(),
        rate_limited.as_deref(),
        *newly_idle,
        *completion,
    );
    Some(InteractionCells {
        lead: main,
        name_index,
        tags: tag_cell,
        branch: branch_cell,
        model,
        labels,
    })
}

/// A worktree's name, or the `↑` standing for it, on a row sharing the
/// cursor's worktree: lifted as the cursor's own row lifts it, and spared the
/// fading of the rows around it.
fn shared_worktree_style() -> Style {
    blended_style(
        Style::default().fg(theme::DIRECTORY_NAME),
        theme::TEXT,
        SELECTED_BRIGHTEN,
    )
    .add_modifier(Modifier::BOLD)
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
mod tests;
