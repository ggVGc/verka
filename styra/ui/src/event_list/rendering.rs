use super::*;
use crate::chrome::{all_events_title, focused_interaction_block, StopTone};
use crate::code::{code_block_lines, is_error_diagnostic};
use crate::footer::{message_text_color, tag_color};
use crate::markdown::{
    markdown_block_render, parse_inline_spans_with_highlight, structural_indent, EntryIndex,
    LinkDisplay,
};
use crate::render_cache::{Memo, Weigh};
use crate::search::{self};
use crate::text::{self, wrap_line};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::cell::RefCell;
use std::time::Duration;
use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};
use styra_protocol::{Contract, QueuedMessage};

const DETAIL_INDENT: &str = "    ";

/// What every row of the list renders the same way: the protocol that reads
/// its events, and the operator's display choices over the result.
///
/// Passed as one value because it is one thing — how this list is being read —
/// and because each row is built twice, once for its height and once clipped
/// to the space left for it.
#[derive(Clone, Copy)]
pub struct EntryRender<'a> {
    pub protocol: Protocol,
    pub links: LinkDisplay,
    /// The term whose matching words are marked, once enough of one has been
    /// typed. See [`crate::search::term`].
    pub search: Option<&'a str>,
}

pub fn render_entry_log(
    frame: &mut Frame,
    view: &EntryLogView<'_>,
    area: Rect,
) -> EntryLogFeedback {
    use ratatui::widgets::{Block, Borders};
    let (focus, close) = (&view.focus_key, &view.close_key);
    let (border, title) = if view.focused {
        (
            theme::ACCENT,
            format!(" entry log · {focus}: back to the list · {close}: close "),
        )
    } else {
        (
            theme::INACTIVE,
            format!(" entry log · follows selection · {focus}: read it · {close}: close "),
        )
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Span::styled(title, Style::default().fg(theme::MUTED_TEXT)));
    if view.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  nothing in the interaction log yet",
                Style::default().fg(theme::MUTED_TEXT),
            )))
            .block(block),
            area,
        );
        return EntryLogFeedback::default();
    }
    block = block.title_bottom(
        Line::from(Span::styled(
            format!(
                " {} {} ",
                view.entries.len(),
                if view.entries.len() == 1 {
                    "entry"
                } else {
                    "entries"
                }
            ),
            Style::default().fg(theme::MUTED_TEXT),
        ))
        .right_aligned(),
    );
    let width = usize::from(area.width.saturating_sub(2));
    let viewport = usize::from(area.height.saturating_sub(2));
    // The pane is not searched: `/` belongs to the list above it, and marking
    // the same words twice on one screen would say nothing more.
    let entry_render = EntryRender {
        protocol: view.protocol,
        links: view.links,
        search: None,
    };
    let items = (0..view.entries.len()).map(|index| {
        entry_item(
            &entry_at(&view.entries, index),
            width,
            viewport,
            entry_render,
        )
    });
    let limit = view
        .entries
        .len()
        .saturating_sub(viewport)
        .min(usize::from(u16::MAX)) as u16;
    let mut effective = view.requested_scroll.min(limit);
    // Every row here is one line, so keeping the cursor on screen is a matter
    // of holding the offset to the window around it. The cursor is the
    // operator's place in the pane, so it wins over a stale scroll offset.
    if let Some(cursor) = view.entries.iter().position(|entry| entry.selected) {
        let cursor = cursor.min(usize::from(u16::MAX)) as u16;
        let page = viewport.max(1) as u16 - 1;
        if cursor < effective {
            effective = cursor;
        } else if cursor > effective.saturating_add(page) {
            effective = cursor.saturating_sub(page);
        }
        effective = effective.min(limit);
    }
    let mut state = ListState::default();
    *state.offset_mut() = usize::from(effective);
    frame.render_stateful_widget(List::new(items).block(block), area, &mut state);
    EntryLogFeedback {
        limit,
        effective_scroll: state.offset().min(usize::from(u16::MAX)) as u16,
    }
}

pub fn render(frame: &mut Frame, view: &EventListView<'_>, area: Rect) -> EventListFeedback {
    let usage = view
        .usage
        .map(|(input, output, cached)| {
            format!(
                " in {} · out {} · cached {} ",
                format_tokens(input),
                format_tokens(output),
                format_tokens(cached)
            )
        })
        .unwrap_or_default();
    let mut block =
        focused_interaction_block(&view.chrome).title_bottom(Line::from(usage).right_aligned());
    // The interaction's state leads the bottom border, ahead of the fleet's
    // tally.
    let mut status = vec![Span::raw(" ")];
    status.extend(crate::chrome::status_spans(&view.chrome));
    let activity = activity_spans(view.activity);
    if activity.is_empty() {
        status.push(Span::raw(" "));
    }
    status.extend(activity);
    block = block.title_bottom(Line::from(status));
    if view.all_events {
        block = block.title_bottom(all_events_title(view.show_minor).right_aligned());
    }
    if view.uncommitted_changes {
        block = crate::chrome::uncommitted_title(block);
    }
    if view.read_only {
        block = crate::chrome::read_only_title(block);
    }
    if let Some(search) = search::prompt(&view.search) {
        block = block.title_bottom(search);
    }

    if view.entries.is_empty() {
        // Before anything is launched, the empty list is the start screen: the
        // one moment the agent, model, and effort are still open, so it says
        // what they are and how to change them instead of only waiting.
        let mut lines = if view.can_configure_launch {
            vec![
                Line::from(vec![
                    Span::styled(
                        "  launching with ",
                        Style::default().fg(theme::MUTED_TEXT),
                    ),
                    Span::styled(
                        view.selection_name.as_str(),
                        Style::default()
                            .fg(theme::ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    "  press L to choose the default agent, model, and effort — or i to write the first message",
                    Style::default().fg(theme::MUTED_TEXT),
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "  waiting for the agent — press i to send a message",
                Style::default().fg(theme::MUTED_TEXT),
            ))]
        };
        lines.extend(queued_lines(
            view.queued,
            area.width.saturating_sub(2) as usize,
        ));
        frame.render_widget(Paragraph::new(lines).block(block), area);
        return EventListFeedback::default();
    }

    let width = area.width.saturating_sub(2) as usize;
    let entry_render = EntryRender {
        protocol: view.protocol,
        links: view.links,
        search: view.search.term(),
    };
    let outer = area;
    let inner = block.inner(area);
    // A branch's log begins at its `branched from` marker, which stays locked
    // at the top of the pane: the rest of the list scrolls beneath it, in its
    // own numbering one behind the caller's. Held to half the pane, so an
    // expanded marker cannot crowd out the list it heads.
    let pinned = pinned_marker(&view.entries);
    let skipped = usize::from(pinned.is_some());
    let pinned_rows = pinned.map_or(0, |marker| {
        let max_rows = (usize::from(inner.height) / 2).max(1);
        let rows = entry_rows_with_max_rows(marker, width, max_rows, entry_render);
        let height = rows.len().min(usize::from(inner.height)) as u16;
        frame.render_widget(Paragraph::new(rows), Rect { height, ..inner });
        height
    });
    let area = Rect {
        y: inner.y + pinned_rows,
        height: inner.height - pinned_rows,
        ..inner
    };
    let viewport_height = usize::from(area.height);
    let rest;
    let entries: &[EventEntry<'_>] = match pinned {
        Some(_) => {
            rest = beneath_pinned(&view.entries[1..]);
            &rest
        }
        None => &view.entries,
    };
    let requested_offset = view.requested_offset.saturating_sub(skipped);
    // Built on demand: the offset math below asks about a window of items, not
    // all of them. See [`LazyItems`] and [`Heights`].
    //
    // The status tail is one of them. Including it is what lets the scroll
    // decision see the row waiting below the last entry — otherwise moving
    // past a tall entry can look attractive merely because the algorithm
    // cannot see what is under it.
    let mut items = LazyItems {
        entries,
        tail: ListItem::new(tail_lines(&view.status, view.queued, width)),
        built: (0..=entries.len()).map(|_| None).collect(),
        width,
        viewport_height,
        render: entry_render,
    };
    // No `highlight_style`: it applies to the whole selected row as one
    // unit, so an expanded entry's detail body would be filled — and forced
    // bold — right along with its summary line, with no way to exempt it.
    // `entry_item` paints the backdrop on the summary row alone instead, so
    // the selection reads as a single line rather than as a block.
    let mut state = ListState::default();
    // With the pinned marker under the cursor, the list beneath it is placed
    // as for its first entry: the top of the branch's own work in view.
    let selected = entries.iter().position(|entry| entry.selected);
    let position = selected.or_else(|| {
        pinned
            .is_some_and(|marker| marker.selected && !entries.is_empty())
            .then_some(0)
    });
    // Re-wrapped to a new width, the entries above the selection take a
    // different number of rows, so the anchor last drawn no longer puts it
    // where it was. Start instead from the one that does, and let the anchor
    // below treat that as where the view already is.
    let (requested_offset, requested_row_offset) = match (view.rendered_selection_row, position) {
        (Some(last), Some(_)) if last.width != width => selection_top_anchor(
            position,
            &mut items,
            last.row.saturating_sub(usize::from(pinned_rows)),
        ),
        _ => (requested_offset, view.requested_row_offset),
    };
    let anchored_position = if view.anchor == ListAnchor::Free {
        None
    } else {
        position
    };
    let tail_in_view = view.anchor == ListAnchor::Selection
        && selected.is_some_and(|selected| selected + 1 == entries.len());
    let response_space = tail_in_view
        && matches!(
            entries[position.unwrap()].event,
            AgentEvent::UserMessage { .. }
        );
    let (offset, row_offset) = if response_space || (tail_in_view && requested_row_offset > 0) {
        tail_space_anchor(
            requested_offset,
            requested_row_offset,
            position.unwrap(),
            &mut items,
            viewport_height,
            if response_space { 3 } else { 0 },
        )
    } else if view.anchor == ListAnchor::Centered {
        selection_center_anchor(position, &mut items, viewport_height)
    } else if let Some(kept) = (view.anchor == ListAnchor::Kept)
        .then_some(anchored_position)
        .flatten()
        .and_then(|selected| {
            kept_anchor(
                requested_offset,
                requested_row_offset,
                selected,
                &mut items,
                viewport_height,
            )
        })
    {
        kept
    } else if view.anchor != ListAnchor::Free {
        (
            list_offset_with_scrolloff(
                requested_offset,
                anchored_position,
                &mut items,
                viewport_height,
                view.moved_backward,
            ),
            0,
        )
    } else {
        normalize_scroll_anchor(
            requested_offset,
            requested_row_offset,
            view.scroll_delta,
            &mut items,
            entries.len(),
        )
    };
    let selection_row = position
        .filter(|position| *position < entries.len())
        .and_then(|position| {
            rows_above_selection(offset, row_offset, position, &mut items, viewport_height)
        })
        .map(|rows| SelectionRow {
            width,
            row: usize::from(pinned_rows) + rows,
        });
    let selection_overflow = selected.and_then(|selected| {
        selection_overflow(offset, row_offset, selected, &mut items, viewport_height)
    });
    // Only what the viewport can show is built, and only that is handed over.
    // A session of any length therefore costs one screen of rendering per
    // frame rather than its whole history — see [`LazyItems::into_window`].
    let mut items = items.into_window(offset, anchored_position, row_offset);
    if row_offset > 0 && offset < entries.len() {
        items[0] = entry_item_slice(
            &entry_at(entries, offset),
            width,
            row_offset,
            usize::MAX,
            entry_render,
        );
    }
    clip_boundary_entry(
        &mut items,
        entries,
        offset,
        viewport_height,
        width,
        entry_render,
        row_offset,
    );
    let visible_selection = anchored_position
        .filter(|position| *position >= offset && *position < offset + items.len())
        .map(|position| position - offset);
    // Taken before the window is handed to the list, to find the selected
    // entry's rows once it is drawn.
    let window_heights = items.iter().map(ListItem::height).collect::<Vec<_>>();
    let list = List::new(items);
    // Both indices are rebased onto the window, which begins at `offset`: to
    // ratatui this is the whole list, seen from the top.
    //
    // `ListState::select(None)` also resets the offset to zero, and this list
    // renders with nothing selected whenever the selected entry is one the
    // filters hide. Assign the field directly so that a computed offset is
    // never thrown away: the offset this render reports back is persisted, so
    // a zero here would scroll the interaction log to the top and keep it
    // there rather than flickering for one frame.
    *state.selected_mut() = visible_selection;
    *state.offset_mut() = 0;
    frame.render_stateful_widget(list, area, &mut state);
    // Whether the top of the list is in view is otherwise only told by its
    // first entry, which reads like any other: the top border says so instead
    // whenever anything is scrolled away above. A pinned marker is not counted:
    // it is always in view.
    let above = offset + state.offset();
    if above > 0 || row_offset > 0 {
        block = block.title(scrolled_title(above).right_aligned());
    }
    frame.render_widget(block, outer);
    // The selected entry itself, not the one the view is placed for: with the
    // pinned marker selected, that is the marker's rows.
    let edge_rows = if pinned.is_some_and(|marker| marker.selected) {
        Some(0..usize::from(pinned_rows))
    } else {
        // Within the window, whose first `state.offset()` items ratatui may
        // have scrolled past.
        let first = state.offset();
        selected
            .and_then(|selected| selected.checked_sub(offset))
            .filter(|index| (first..window_heights.len()).contains(index))
            .map(|index| {
                let start = window_heights[first..index].iter().sum::<usize>();
                let end = start + window_heights[index];
                let pinned = usize::from(pinned_rows);
                pinned + start.min(viewport_height)..pinned + end.min(viewport_height)
            })
    };
    if let (Some(rows), true) = (edge_rows, inner.x > outer.x) {
        crate::chrome::selected_row_edge(
            frame,
            outer.x,
            inner.y + rows.start as u16..inner.y + rows.end as u16,
        );
    }
    EventListFeedback {
        // Back into the caller's numbering. Ratatui only ever moves the offset
        // forward from where it was put, so this stays within the window.
        effective_offset: offset + state.offset() + skipped,
        effective_row_offset: row_offset,
        selection_row,
        selection_overflow,
    }
}

/// How far the selected entry runs past each edge of the viewport drawn from
/// `offset`. `None` when none of it is in view.
fn selection_overflow(
    offset: usize,
    row_offset: usize,
    selected: usize,
    heights: &mut impl Heights,
    viewport_height: usize,
) -> Option<SelectionOverflow> {
    if selected >= heights.len() {
        return None;
    }
    let height = heights.height(selected);
    if selected == offset {
        let row_offset = row_offset.min(height.saturating_sub(1));
        return Some(SelectionOverflow {
            above: row_offset,
            below: (height - row_offset).saturating_sub(viewport_height),
        });
    }
    let top = rows_above_selection(offset, row_offset, selected, heights, viewport_height)?;
    Some(SelectionOverflow {
        above: 0,
        below: (top + height).saturating_sub(viewport_height),
    })
}

/// The requested anchor, as long as the whole selected entry is visible from
/// it. A selection running past the foot of the view — off it entirely, or cut
/// short with its `… more rows` line — scrolls it down only as far as it takes
/// to show the whole entry, starting partway through a tall entry above if
/// need be, rather than putting the selection at the top. `None` when the view
/// has to move up to show the selection.
fn kept_anchor(
    offset: usize,
    row_offset: usize,
    selected: usize,
    heights: &mut impl Heights,
    viewport_height: usize,
) -> Option<(usize, usize)> {
    if offset >= heights.len() || selected < offset {
        return None;
    }
    let row_offset = row_offset.min(heights.height(offset).saturating_sub(1));
    if selected == offset && row_offset > 0 {
        return None;
    }
    // An entry taller than the pane shows its first rows.
    let height = heights.height(selected);
    let required = if height >= viewport_height {
        (selected, 0)
    } else {
        selection_top_anchor(Some(selected), heights, viewport_height - height)
    };
    Some((offset, row_offset).max(required))
}

/// How many rows of the view, anchored at `offset` and `row_offset`, are
/// above the selected entry; `None` when it is not in view. Stops at the foot
/// of the view rather than measuring everything up to a selection far below.
fn rows_above_selection(
    offset: usize,
    row_offset: usize,
    selected: usize,
    heights: &mut impl Heights,
    viewport_height: usize,
) -> Option<usize> {
    if selected < offset {
        return None;
    }
    let mut rows = 0usize;
    for index in offset..selected {
        rows += heights.height(index);
        if rows.saturating_sub(row_offset) >= viewport_height {
            return None;
        }
    }
    Some(rows.saturating_sub(row_offset))
}

/// The latest viewport anchor that leaves no more than `maximum_lines` before
/// the selected entry. The anchor can begin partway through a wrapped entry,
/// which is why it returns both an item and a rendered-row offset.
fn selection_top_anchor(
    selected: Option<usize>,
    heights: &mut impl Heights,
    maximum_lines: usize,
) -> (usize, usize) {
    let Some(selected) = selected else {
        return (0, 0);
    };
    let mut entry = selected;
    let mut remaining = maximum_lines;
    while entry > 0 {
        let previous = entry - 1;
        let height = heights.height(previous);
        if height >= remaining {
            return (previous, height - remaining);
        }
        remaining -= height;
        entry = previous;
    }
    (0, 0)
}

/// Place the selected entry in the middle of the viewport, with as much
/// history above it as below. An entry too tall to be centered starts at the
/// top instead.
fn selection_center_anchor(
    selected: Option<usize>,
    heights: &mut impl Heights,
    viewport_height: usize,
) -> (usize, usize) {
    let Some(selected) = selected else {
        return (0, 0);
    };
    match viewport_height.saturating_sub(heights.height(selected)) / 2 {
        0 => (selected, 0),
        above => selection_top_anchor(Some(selected), heights, above),
    }
}

/// Leave a few rows below a newly submitted message for the response. Move
/// only as far as needed, including partway through a wrapped history entry.
/// If the message already fits with that space, preserve the current anchor.
/// Subsequent output retains that partial anchor without reserving extra rows.
fn tail_space_anchor(
    offset: usize,
    row_offset: usize,
    selected: usize,
    heights: &mut impl Heights,
    viewport_height: usize,
    response_rows: usize,
) -> (usize, usize) {
    let message_rows = viewport_height.saturating_sub(response_rows).max(1);
    let required = selection_top_anchor(Some(selected + 1), heights, message_rows);
    if offset > selected {
        return required;
    }
    let current = (
        offset,
        row_offset.min(heights.height(offset).saturating_sub(1)),
    );
    current.max(required)
}

/// Ratatui's `List` only renders complete items. If the next expanded entry is
/// taller than the rows left at the bottom of the viewport, it would therefore
/// disappear entirely even though some of its text could be shown. Rebuild
/// that boundary entry with the actual remaining row budget. When it is the
/// final entry, retain a row for the status tail whenever there is room for
/// both its summary and the tail.
/// `items` is the window beginning at `offset`, so a position in it names the
/// entry `offset` further along — see [`LazyItems::into_window`].
fn clip_boundary_entry(
    items: &mut [ListItem<'static>],
    entries: &[EventEntry<'_>],
    offset: usize,
    viewport_height: usize,
    width: usize,
    entry_render: EntryRender<'_>,
    first_row_offset: usize,
) {
    let mut remaining = viewport_height;
    let mut boundary = None;
    for (position, item) in items.iter().enumerate() {
        let height = item.height();
        if height > remaining {
            boundary = Some(position);
            break;
        }
        remaining -= height;
    }
    if let Some(position) = boundary {
        let item_index = offset + position;
        if remaining == 0 || item_index >= entries.len() {
            return;
        }

        let entry = &entry_at(entries, item_index);
        let is_last_entry = item_index + 1 == entries.len();
        let max_rows = if is_last_entry && remaining > 1 {
            remaining - 1
        } else {
            remaining
        };
        items[position] = if position == 0 && first_row_offset > 0 {
            entry_item_slice(entry, width, first_row_offset, max_rows, entry_render)
        } else {
            entry_item_with_max_rows(entry, width, max_rows, entry_render)
        };
    }
}

/// The list's items, each built the first time it is asked for.
///
/// Indexed like the list it stands for, with the status tail last — so index
/// `entries.len()` is the tail, and [`Heights::len`] counts it.
///
/// Building an item is how its height is discovered: a row's height is the
/// number of rows it wraps to, which is only known once it is rendered. So the
/// result is kept, and [`Self::into_items`] hands back what was built without
/// building it twice.
struct LazyItems<'a> {
    entries: &'a [EventEntry<'a>],
    tail: ListItem<'static>,
    built: Vec<Option<ListItem<'static>>>,
    width: usize,
    viewport_height: usize,
    render: EntryRender<'a>,
}

/// The `branched from` marker a branch's log begins with, when this list is
/// one: [`render`] locks it at the top of the pane, so where the conversation
/// came from stays in sight however far the operator scrolls.
fn pinned_marker<'e, 'a>(entries: &'e [EventEntry<'a>]) -> Option<&'e EventEntry<'a>> {
    entries.first().filter(|entry| {
        matches!(
            entry.event,
            AgentEvent::Branched {
                direction: styra_protocol::event::BranchDirection::From,
                ..
            }
        )
    })
}

/// The top-border marker for a list scrolled away from its top: how many
/// entries are out of sight above, or only that some of the first one is.
fn scrolled_title(above: usize) -> Line<'static> {
    let text = match above {
        0 => " ▲ more above ".to_owned(),
        1 => " ▲ 1 entry above ".to_owned(),
        above => format!(" ▲ {above} entries above "),
    };
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    ))
}

/// The entries under a pinned marker, numbered from the first of them. That
/// one still sits directly beneath the marker, so a marker there shares its
/// lower border as it would in the unpinned list — see [`entry_at`].
fn beneath_pinned<'a>(entries: &[EventEntry<'a>]) -> Vec<EventEntry<'a>> {
    let mut entries = entries.to_vec();
    if let Some(first) = entries.first_mut() {
        first.branch_follows_marker |= branch_border(first.event).is_some();
    }
    entries
}

fn entry_at<'a>(entries: &[EventEntry<'a>], index: usize) -> EventEntry<'a> {
    let mut entry = entries[index].clone();
    entry.branch_follows_marker |= index > 0
        && branch_border(entries[index - 1].event).is_some()
        && branch_border(entry.event).is_some();
    entry
}

impl LazyItems<'_> {
    fn item(&mut self, index: usize) -> &ListItem<'static> {
        self.built[index].get_or_insert_with(|| match self.entries.get(index) {
            // Whole, however tall: the viewport scrolls through an entry
            // taller than itself, so the rows past the pane's height must be
            // there to reach. What is handed to `List` is cut to fit — see
            // [`clip_boundary_entry`].
            Some(_) => entry_item_with_max_rows(
                &entry_at(self.entries, index),
                self.width,
                usize::MAX,
                self.render,
            ),
            None => self.tail.clone(),
        })
    }

    /// The items the list will actually draw, starting at `start`: from there
    /// until the viewport is full, and never one above it.
    ///
    /// Ratatui's `List` applies the offset to the items it is given, so
    /// handing it only this window means rebasing the offset and the selection
    /// to it — see [`render`]. It reads nothing outside the window: with no
    /// `scroll_padding` set its `index_to_display` is the selected index, and
    /// the offset is never past the selection, so its one backward-walking
    /// branch cannot fire. Forward it stops as soon as the viewport is full.
    ///
    /// The first item that does not fit whole is still included: it is the one
    /// [`clip_boundary_entry`] rebuilds to the rows actually left for it. So is
    /// anything up to the selection, which ratatui walks forward to.
    fn into_window(
        mut self,
        start: usize,
        selected: Option<usize>,
        first_row_offset: usize,
    ) -> Vec<ListItem<'static>> {
        let mut used = 0usize;
        let mut end = start;
        while end < Heights::len(&self) {
            let height = self.height(end);
            used = used.saturating_add(if end == start {
                height.saturating_sub(first_row_offset)
            } else {
                height
            });
            end += 1;
            if used > self.viewport_height && selected.is_none_or(|selected| end > selected) {
                break;
            }
        }
        self.built[start..end]
            .iter_mut()
            .map(|item| item.take().expect("every item of the window was built"))
            .collect()
    }
}

impl Heights for LazyItems<'_> {
    fn len(&self) -> usize {
        self.built.len()
    }

    fn height(&mut self, index: usize) -> usize {
        self.item(index).height()
    }
}

/// How tall each item of the list is, asked one item at a time.
///
/// The offset math used to take every height as a slice, which meant the
/// caller had to render the whole session to compute a scroll position — and
/// that is the reason the list was rebuilt in full on every frame, not
/// anything ratatui needs. Nothing below actually reads more than a window:
/// the walks start at the offset and stop as soon as the viewport is full,
/// and the backward walk is bounded by `margin`. Asking one at a time makes
/// that demand explicit, so a provider can build only what is asked for.
///
/// `&mut self`, because the interesting implementation renders an item to
/// find out how tall it is and keeps the result.
pub(crate) trait Heights {
    /// How many items there are. Known up front — it is the length of the
    /// list, not of anything rendered.
    fn len(&self) -> usize;

    /// The height of one item, in rendered rows.
    fn height(&mut self, index: usize) -> usize;

    /// Total height of `range`, which must lie within [`Self::len`].
    fn total(&mut self, range: std::ops::Range<usize>) -> usize {
        range.map(|index| self.height(index)).sum()
    }
}

impl Heights for &[usize] {
    fn len(&self) -> usize {
        <[usize]>::len(self)
    }

    fn height(&mut self, index: usize) -> usize {
        self[index]
    }
}

/// Resolve a signed rendered-row movement into an entry and a row within it.
/// The last entry is the lower bound: the status tail still renders beneath
/// it, but is not allowed to become a mostly-empty viewport anchor of its own.
fn normalize_scroll_anchor(
    requested_entry: usize,
    requested_row: usize,
    delta: i32,
    heights: &mut impl Heights,
    entry_count: usize,
) -> (usize, usize) {
    if entry_count == 0 {
        return (0, 0);
    }
    let last = entry_count - 1;
    let maximum = bottom_scroll_anchor(heights, entry_count, 5);
    let mut entry = requested_entry.min(last);
    let mut row = requested_row.min(heights.height(entry).saturating_sub(1));
    if (entry, row) > maximum {
        (entry, row) = maximum;
    }

    if delta >= 0 {
        let mut remaining = delta as usize;
        while remaining > 0 {
            let available = heights.height(entry).saturating_sub(row + 1);
            if remaining <= available {
                row += remaining;
                break;
            }
            if entry == last {
                row += available;
                break;
            }
            remaining = remaining.saturating_sub(available + 1);
            entry += 1;
            row = 0;
        }
    } else {
        let mut remaining = delta.unsigned_abs() as usize;
        while remaining > 0 {
            if remaining <= row {
                row -= remaining;
                break;
            }
            if entry == 0 {
                row = 0;
                break;
            }
            remaining = remaining.saturating_sub(row + 1);
            entry -= 1;
            row = heights.height(entry).saturating_sub(1);
        }
    }
    (entry, row).min(maximum)
}

/// Latest top-of-viewport anchor that leaves `minimum_lines` interaction
/// lines on screen. When the interaction is shorter, its first line remains
/// the latest possible anchor so every line stays visible.
fn bottom_scroll_anchor(
    heights: &mut impl Heights,
    entry_count: usize,
    minimum_lines: usize,
) -> (usize, usize) {
    if entry_count == 0 || minimum_lines == 0 {
        return (0, 0);
    }
    let mut entry = entry_count - 1;
    let mut remaining = minimum_lines;
    loop {
        let height = heights.height(entry);
        if remaining <= height {
            return (entry, height - remaining);
        }
        remaining -= height;
        if entry == 0 {
            return (0, 0);
        }
        entry -= 1;
    }
}

/// Keep the selected item within a small margin of the viewport edges, like
/// vim's `scrolloff`, without throwing away visible content just to preserve
/// that margin. Heights are rendered rows rather than item counts so wrapped
/// summaries and expanded details do not break the calculation.
///
/// Only asks [`Heights`] about the items around the viewport — see there.
fn list_offset_with_scrolloff(
    current: usize,
    selected: Option<usize>,
    heights: &mut impl Heights,
    viewport_height: usize,
    moved_backward: bool,
) -> usize {
    let Some(selected) = selected else {
        return current.min(heights.len().saturating_sub(1));
    };
    if viewport_height == 0 {
        return selected;
    }

    let margin = 2.min(viewport_height.saturating_sub(1) / 2);
    let mut offset = current.min(selected);

    // First do only the scrolling required to make the complete selection
    // visible. In particular, use the whole viewport here rather than
    // reserving the preferred margin: a tall preceding message and a short
    // selected entry may fit perfectly together.
    let mut rows_through_selection = heights.total(offset..selected + 1);
    while offset < selected && rows_through_selection > viewport_height {
        rows_through_selection = rows_through_selection.saturating_sub(heights.height(offset));
        offset += 1;
    }

    // Moving upward may have put the selection against the top. Pull earlier
    // items back in while they fit and do not reduce the number of occupied
    // rows (they can displace content at the bottom of the viewport).
    while moved_backward && offset > 0 && rows_before_selection(offset, selected, heights) < margin
    {
        let candidate = offset - 1;
        if heights.total(candidate..selected + 1) > viewport_height
            || visible_rows(candidate, heights, viewport_height)
                < visible_rows(offset, heights, viewport_height)
        {
            break;
        }
        offset = candidate;
    }

    // Prefer the same margin below the selection when advancing. It is only
    // a preference: if dropping the first item would leave fewer rows filled,
    // keep the denser viewport. This is what prevents a long message from
    // disappearing as soon as the following one-line event is selected.
    while offset < selected
        && rows_after_selection(offset, selected, heights, viewport_height) < margin
    {
        let candidate = offset + 1;
        if visible_rows(candidate, heights, viewport_height)
            < visible_rows(offset, heights, viewport_height)
            || rows_after_selection(candidate, selected, heights, viewport_height)
                <= rows_after_selection(offset, selected, heights, viewport_height)
        {
            break;
        }
        offset = candidate;
    }
    offset
}

/// Rows the viewport actually shows starting at `offset`. Stops at the first
/// item that would not fit whole, so it never looks past the viewport.
fn visible_rows(offset: usize, heights: &mut impl Heights, viewport_height: usize) -> usize {
    let mut used = 0usize;
    for index in offset..heights.len() {
        let height = heights.height(index);
        if used.saturating_add(height) > viewport_height {
            break;
        }
        used += height;
    }
    used
}

fn rows_before_selection(offset: usize, selected: usize, heights: &mut impl Heights) -> usize {
    heights.total(offset..selected)
}

/// Rows below the selection that the viewport shows. Stops with the viewport,
/// as [`visible_rows`] does.
fn rows_after_selection(
    offset: usize,
    selected: usize,
    heights: &mut impl Heights,
    viewport_height: usize,
) -> usize {
    let mut used = 0usize;
    let mut after = 0usize;
    for index in offset..heights.len() {
        let height = heights.height(index);
        if used.saturating_add(height) > viewport_height {
            break;
        }
        used += height;
        if index > selected {
            after += height;
        }
    }
    after
}

/// Gaps shorter than this are not named: while output is streaming the figure
/// would flicker between `0s` and `1s` and say nothing. Only a real pause is
/// worth reporting.
const QUIET_THRESHOLD: Duration = Duration::from_secs(3);

/// A reason appended to a state, or nothing at all where the state explains
/// itself.
fn why(reason: &Option<String>) -> String {
    reason
        .as_ref()
        .map(|reason| format!(" · {reason}"))
        .unwrap_or_default()
}

/// The line under the last entry saying what the interaction is doing; the
/// overview draws the same one under each tile's messages.
pub fn status_tail(status: &EventListStatus) -> Line<'static> {
    let (text, color) = match status {
        EventListStatus::Pending => (
            "  … waiting for your first message".to_string(),
            theme::INACTIVE,
        ),
        EventListStatus::Running {
            elapsed,
            quiet,
            events,
        } => (running_tail(*elapsed, *quiet, *events), theme::RUNNING),
        // Idle carries no elapsed figure: nothing is happening, so a
        // climbing counter only draws the eye to a number that means nothing.
        EventListStatus::Idle { reason, failed } => (
            format!("  ── idle{} ──", why(reason)),
            if *failed {
                theme::ERROR
            } else {
                theme::SUCCESS
            },
        ),
        EventListStatus::Background { elapsed } => (
            format!(
                "  ── idle {} · background work still running ──",
                format_duration(*elapsed)
            ),
            theme::WARNING,
        ),
        // Integration is the outcome rather than a way of stopping; its reason
        // says so on its own.
        EventListStatus::Stopped {
            reason: Some(reason),
            tone: tone @ (StopTone::Integrated | StopTone::IntegrationPending),
            ..
        } => (format!("  ── {reason} ──"), tone.color()),
        EventListStatus::Stopped { reason, tone, .. } => {
            (format!("  ── stopped{} ──", why(reason)), tone.color())
        }
        _ => return Line::default(),
    };
    Line::from(Span::styled(text, Style::default().fg(color)))
}

/// Queued messages beyond this many are counted rather than listed. The tail
/// is one list item, and the list draws only items that fit whole, so a long
/// queue would otherwise take the status line off the screen with it.
const MAX_QUEUED_ROWS: usize = 5;

/// The status line, followed by the messages waiting to be sent after it.
fn tail_lines(
    status: &EventListStatus,
    queued: &[QueuedMessage],
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![status_tail(status)];
    lines.extend(queued_lines(queued, width));
    lines
}

/// One row per queued message, clipped to the pane: the message box lists
/// them in full, so here they only have to say what is waiting, and in what
/// order.
fn queued_lines(queued: &[QueuedMessage], width: usize) -> Vec<Line<'static>> {
    let muted = Style::default().fg(theme::INACTIVE);
    let mut lines: Vec<Line<'static>> = queued
        .iter()
        .take(MAX_QUEUED_ROWS)
        .map(|message| {
            let label = match message.contract {
                Some(contract) => format!("  ⧗ queued ({}) » ", contract.as_str()),
                None => "  ⧗ queued » ".to_owned(),
            };
            // One row each, so a multi-line message reads as its first line.
            let text = message.text.lines().next().unwrap_or_default().to_owned();
            truncate_line(
                Line::from(vec![
                    Span::styled(label, muted),
                    Span::styled(text, Style::default().fg(theme::MUTED_TEXT)),
                ]),
                width,
                false,
            )
        })
        .collect();
    if queued.len() > MAX_QUEUED_ROWS {
        lines.push(Line::from(Span::styled(
            format!("  ⧗ +{} more queued", queued.len() - MAX_QUEUED_ROWS),
            muted,
        )));
    }
    lines
}

/// The tail of a running turn: a spinner, how long the turn has been going,
/// and — once the agent has been quiet long enough for that to be a question —
/// how long since anything last came back from it.
fn running_tail(elapsed: Duration, quiet: Option<Duration>, events: usize) -> String {
    let mut text = format!(
        "  {} working {}",
        crate::interactions::running_indicator(events),
        format_duration(elapsed)
    );
    if let Some(gap) = quiet.filter(|gap| *gap >= QUIET_THRESHOLD) {
        text.push_str(&format!(" · last update {} ago", format_duration(gap)));
    }
    text
}

/// `viewport_height` is the list's own visible row count. An expanded entry is
/// clamped to fit inside it: `List` refuses to draw an item taller than the
/// viewport at all, and — because it also evicts everything around it while
/// making room — one over-long message would blank the whole list rather than
/// merely overflow it. The clipped tail is not lost; the preview panel (`p`)
/// shows the entry in full and scrolls.
pub fn entry_item(
    entry: &EventEntry<'_>,
    width: usize,
    viewport_height: usize,
    render: EntryRender<'_>,
) -> ListItem<'static> {
    entry_item_with_max_rows(
        entry,
        width,
        viewport_height.saturating_sub(1).max(1),
        render,
    )
}

/// Everything that shapes a [`build_entry_rows`] result.
///
/// The entry contributes its [`EntryVersion`] rather than its event: the
/// version is what says whether the event is still the one that was rendered,
/// and comparing it is a `u64` and a `u32` rather than a hash of the whole
/// message. `has_detail` and `contract` are derived from the same row, so the
/// version covers them too — but they are cheap and keying them explicitly
/// means this does not depend on that staying true.
///
/// `selected` is part of the key rather than something applied to a finished
/// row afterwards. It reaches further into the build than it looks:
/// [`selected_summary_line`] rewrites the summary's first span *before* the
/// row is truncated and wrapped, so lifting it out would mean reasoning about
/// where that span ended up. A cursor move rebuilds the two rows it touches
/// instead, which is two rows out of a session.
#[derive(PartialEq, Eq, Hash)]
struct RowKey {
    version: EntryVersion,
    width: usize,
    max_rows: usize,
    expanded: bool,
    has_detail: bool,
    selected: bool,
    contract: Option<Contract>,
    protocol: Protocol,
    links: LinkDisplay,
    link_highlight: Option<EntryIndex>,
    branch_follows_marker: bool,
    selected_branch: Option<usize>,
    search: Option<String>,
    branch: BranchKey,
    branch_provider_switch: Option<(
        styra_protocol::agent::Provider,
        styra_protocol::agent::Provider,
    )>,
}

/// A [`BranchLink`] as [`RowKey`] holds it: owned, and with the whole tree
/// of branches beneath it, since any of them changing changes the rows.
#[derive(PartialEq, Eq, Hash)]
struct BranchKey {
    name: Option<String>,
    state: Option<crate::interactions::InteractionState>,
    message: Option<String>,
    worktree: Option<String>,
    tags: Vec<String>,
    children: Vec<BranchKey>,
}

impl BranchKey {
    fn of(branch: &BranchLink<'_>) -> Self {
        Self {
            name: branch.name.map(str::to_owned),
            state: branch.state.clone(),
            message: branch.message.map(str::to_owned),
            worktree: branch.worktree.map(str::to_owned),
            tags: branch.tags.to_vec(),
            children: branch.children.iter().map(Self::of).collect(),
        }
    }
}

impl Weigh for Vec<Line<'static>> {
    fn weight(&self) -> usize {
        // An entry that renders to nothing still occupies a table slot, and
        // deciding that it does is the work being saved.
        self.len().max(1)
    }
}

thread_local! {
    /// The rows of the list, finished: parsed, styled, wrapped to width, and
    /// marked — everything [`crate::markdown`]'s own cache stops short of.
    ///
    /// The list rebuilds every row it holds on every frame (see [`render`]),
    /// and a frame is drawn per keystroke, so without this, composing a
    /// message re-wraps the whole session once per character. Keyed on the
    /// row's version rather than its text, so a hit costs a small hash
    /// instead of one over every byte of the conversation.
    static ROW_CACHE: RefCell<Memo<RowKey, Vec<Line<'static>>>> = RefCell::new(Memo::default());
}

fn entry_item_with_max_rows(
    entry: &EventEntry<'_>,
    width: usize,
    max_rows: usize,
    render: EntryRender<'_>,
) -> ListItem<'static> {
    ListItem::new(entry_rows_with_max_rows(entry, width, max_rows, render))
}

fn entry_rows_with_max_rows(
    entry: &EventEntry<'_>,
    width: usize,
    max_rows: usize,
    render: EntryRender<'_>,
) -> Vec<Line<'static>> {
    let key = RowKey {
        version: entry.version,
        width,
        max_rows,
        expanded: entry.expanded,
        has_detail: entry.has_detail,
        selected: entry.selected,
        contract: entry.contract.copied(),
        protocol: render.protocol,
        links: render.links,
        link_highlight: entry.link_highlight,
        branch_follows_marker: entry.branch_follows_marker,
        selected_branch: entry.selected_branch,
        search: render.search.map(str::to_owned),
        branch: BranchKey::of(&entry.branch),
        branch_provider_switch: entry.branch_provider_switch,
    };
    ROW_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .get_or_insert_with(key, || build_entry_rows(entry, width, max_rows, render))
    })
}

/// Move a style's foreground `tenths` tenths of the way toward
/// [`theme::INHERITED_FADE_TOWARD`], leaving its background alone.
pub(crate) fn faded_style(style: Style, tenths: u16) -> Style {
    blended_style(style, theme::INHERITED_FADE_TOWARD, tenths)
}

/// Move a style's foreground `tenths` tenths of the way toward `toward`,
/// leaving its background alone.
pub(crate) fn blended_style(style: Style, toward: Color, tenths: u16) -> Style {
    match (style.fg.unwrap_or(theme::TEXT), toward) {
        (Color::Rgb(r, g, b), Color::Rgb(to_r, to_g, to_b)) => {
            let fade = |from: u8, to: u8| {
                let (from, to) = (u16::from(from), u16::from(to));
                ((from * (10 - tenths) + to * tenths) / 10) as u8
            };
            style.fg(Color::Rgb(fade(r, to_r), fade(g, to_g), fade(b, to_b)))
        }
        // A terminal's named colours have no value to blend; let it dim them.
        _ => style.add_modifier(Modifier::DIM),
    }
}

/// `take` rows of the whole entry from row `skip` on, the view having scrolled
/// partway into it. Where rows still follow past them, the last one says how
/// many instead, as a cut entry does at the top.
fn entry_item_slice(
    entry: &EventEntry<'_>,
    width: usize,
    skip: usize,
    take: usize,
    render: EntryRender<'_>,
) -> ListItem<'static> {
    let mut rows = entry_rows_with_max_rows(entry, width, usize::MAX, render)
        .into_iter()
        .skip(skip)
        .collect::<Vec<_>>();
    if rows.len() > take {
        if take > 1 {
            let hidden = rows.len() - (take - 1);
            rows.truncate(take - 1);
            rows.push(more_rows_line(hidden));
        } else {
            rows.truncate(take);
        }
    }
    ListItem::new(rows)
}

/// The last row of an entry cut short at the foot of the pane.
fn more_rows_line(hidden: usize) -> Line<'static> {
    Line::from(Span::styled(
        format!("{DETAIL_INDENT}… {hidden} more rows — press p for the full entry"),
        Style::default().fg(theme::MUTED_TEXT),
    ))
}

/// The rows a conversation entry — a message or a branch marker — is drawn
/// as in the event list, expanded as its conversation-only mode shows it, for
/// a view that shows the conversation outside the list. `branch` is where a
/// marker leads, as [`EventEntry::branch`]. Uncapped: what to cut is up to
/// the view.
pub fn message_rows(
    event: &AgentEvent,
    contract: Option<&Contract>,
    branch: BranchLink<'_>,
    width: usize,
    protocol: Protocol,
    links: LinkDisplay,
) -> Vec<Line<'static>> {
    let entry = EventEntry {
        event,
        // Not rendered through the row cache, so never compared.
        version: EntryVersion { id: 0, revision: 0 },
        expanded: true,
        has_detail: false,
        contract,
        selected: false,
        link_highlight: None,
        branch,
        branch_provider_switch: None,
        branch_follows_marker: false,
        selected_branch: None,
    };
    entry_rows(&entry, width, protocol, links)
}

/// Every row `entry` is drawn as in the event list, for a view that shows
/// another list's entries outside it. Uncapped and uncached: the entry's
/// version is not consulted.
pub fn entry_rows(
    entry: &EventEntry<'_>,
    width: usize,
    protocol: Protocol,
    links: LinkDisplay,
) -> Vec<Line<'static>> {
    let render = EntryRender {
        protocol,
        links,
        search: None,
    };
    build_entry_rows(entry, width, usize::MAX, render)
}

/// [`entry_item_with_max_rows`] proper, behind its cache: every finished row
/// of one entry.
fn build_entry_rows(
    entry: &EventEntry<'_>,
    width: usize,
    max_rows: usize,
    render: EntryRender<'_>,
) -> Vec<Line<'static>> {
    // With the cursor on one of the marker's branches, that row is drawn
    // selected and the marker's own row is not.
    let unselected;
    let entry = if entry.selected_branch.is_some() {
        unselected = EventEntry {
            selected: false,
            ..entry.clone()
        };
        &unselected
    } else {
        entry
    };
    let mut rows = build_branch_entry_rows(entry, width, max_rows, render);
    if finished_branch(entry) && !entry.selected {
        let fade = 3;
        let cursor = Some(theme::SELECTION_BACKGROUND);
        for row in rows.iter_mut().filter(|row| row.style.bg != cursor) {
            row.style = faded_style(row.style, fade);
            for span in &mut row.spans {
                span.style = faded_style(span.style, fade);
            }
        }
    }
    rows
}

fn build_branch_entry_rows(
    entry: &EventEntry<'_>,
    width: usize,
    max_rows: usize,
    render: EntryRender<'_>,
) -> Vec<Line<'static>> {
    let Some(color) = branch_border(entry.event) else {
        return build_entry_content_rows(entry, width, max_rows, render);
    };
    let border_rows = if entry.branch_follows_marker { 1 } else { 2 };
    if width == 0 || max_rows <= border_rows {
        return build_entry_content_rows(entry, width, max_rows, render);
    }
    // The marker's own summary row always fits; its branches take what is
    // left before its detail does.
    let mut children = branch_children_rows(
        &entry.branch.children,
        1,
        width,
        render.protocol,
        &mut entry.selected_branch.map(|at| at + 1),
    );
    children.truncate(max_rows - border_rows - 1);
    let content = build_entry_content_rows(
        entry,
        width,
        max_rows - border_rows - children.len(),
        render,
    );
    let divider = Line::from(Span::styled("─".repeat(width), Style::default().fg(color)));
    let mut rows = Vec::with_capacity(content.len() + children.len() + border_rows);
    if !entry.branch_follows_marker {
        rows.push(divider.clone());
    }
    rows.extend(content);
    rows.extend(children);
    rows.push(divider);
    rows
}

/// The branches taken off where a marker leads, within its band: each one a
/// collapsed `branched to` marker of its own, indented `depth` steps, with its
/// last message under it and its own branches a step further in beneath
/// that. `cursor` counts down the branches
/// still to draw before the selected one, which it reaches at `Some(1)`.
fn branch_children_rows(
    children: &[BranchLink<'_>],
    depth: usize,
    width: usize,
    protocol: Protocol,
    cursor: &mut Option<usize>,
) -> Vec<Line<'static>> {
    let event = AgentEvent::Branched {
        direction: styra_protocol::event::BranchDirection::To,
        session: String::new(),
        name: None,
    };
    let mut rows = Vec::new();
    for child in children {
        let entry = EventEntry {
            event: &event,
            // Drawn as part of its parent's rows, which the cache keys on.
            version: EntryVersion { id: 0, revision: 0 },
            expanded: false,
            has_detail: false,
            contract: None,
            selected: *cursor == Some(1),
            link_highlight: None,
            branch: child.clone(),
            branch_provider_switch: None,
            branch_follows_marker: false,
            selected_branch: None,
        };
        *cursor = cursor.map(|left| left.saturating_sub(1));
        let row = summary_line(&entry, SummaryContext::EventList, protocol);
        let mut row = selected_summary_line(row, false, entry.selected);
        row.spans.insert(0, Span::raw("  ".repeat(depth)));
        let row = with_entry_backdrop(truncate_line(row, width, false), &entry);
        // Its last message beneath it, as the marker's own sits beneath its
        // summary row.
        let message = branch_message_line(child).map(|mut line| {
            line.spans.insert(0, Span::raw("  ".repeat(depth)));
            truncate_line(line, width, false)
        });
        let fade = finished_branch(&entry) && !entry.selected;
        for mut row in std::iter::once(row).chain(message) {
            if fade {
                for span in &mut row.spans {
                    span.style = faded_style(span.style, 3);
                }
            }
            rows.push(row);
        }
        rows.extend(branch_children_rows(
            &child.children,
            depth + 1,
            width,
            protocol,
            cursor,
        ));
    }
    rows
}

fn build_entry_content_rows(
    entry: &EventEntry<'_>,
    width: usize,
    max_rows: usize,
    render: EntryRender<'_>,
) -> Vec<Line<'static>> {
    let EntryRender {
        protocol,
        links,
        search,
    } = render;
    let is_conversation = matches!(
        entry.event,
        AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
    );
    let summary_indent = if is_conversation { 2 } else { 0 };
    let summary = selected_summary_line(
        summary_line(entry, SummaryContext::EventList, protocol),
        is_conversation,
        entry.selected,
    );
    if !entry.expanded {
        // A collapsed entry is always exactly one row: wrapping it would make
        // one long message push the rest of the session off screen, and the
        // available width shrinks whenever the preview pane opens.
        let row = truncate_line(summary, width, entry.has_detail);
        // Marked after the row is cut to width, so a match is only claimed
        // where the operator can actually see it.
        let row = search::highlight_lines(vec![row], search);
        return row
            .into_iter()
            .map(|row| with_entry_backdrop(row, entry))
            .collect();
    }
    let mut lines = vec![summary];
    let mut detail = detail_lines_with_links(entry.event, protocol, links, entry.link_highlight);
    // The first detail line is the summary already shown above. Link focus is
    // rendered on that summary, so it never needs a duplicate body line.
    if !detail.is_empty() {
        detail.remove(0);
    }
    if suspicious_shell_success(entry.event) {
        detail.insert(
            0,
            Line::from(Span::styled(
                format!("{DETAIL_INDENT}reported success; output contains an error diagnostic"),
                Style::default().fg(theme::WARNING),
            )),
        );
    }
    lines.extend(detail);
    let mut wrapped = wrap_log_lines(lines, width, summary_indent);
    // Cut rather than wrapped, so it stays the one line the navigator has.
    if let Some(line) = branch_message_line(&entry.branch) {
        wrapped.push(truncate_line(line, width, false));
    }
    crate::diff::fill_changed_rows(&mut wrapped, width);
    // Bounded to the rows the caller has room for: see [`entry_item`].
    if wrapped.len() > max_rows {
        if max_rows == 1 {
            wrapped.truncate(1);
            if let Some(summary) = wrapped.first_mut() {
                summary
                    .spans
                    .push(Span::styled(" …", Style::default().fg(theme::MUTED_TEXT)));
            }
        } else {
            let hidden = wrapped.len() - (max_rows - 1);
            wrapped.truncate(max_rows - 1);
            wrapped.push(more_rows_line(hidden));
        }
    }
    // After wrapping, so a word broken across two rows is marked on the row it
    // is actually on rather than half-marked on both. The mark is a span
    // style and the selection below is the row's own, so they coexist.
    let mut wrapped = search::highlight_lines(wrapped, search);
    if let Some(row) = wrapped.first_mut() {
        *row = with_entry_backdrop(std::mem::take(row), entry);
    }
    wrapped
}

/// Tint operator messages and conversation changes, and mark a selected row,
/// by backing its first row
/// only. An expanded entry's detail body keeps the plain background, so either
/// cue reads as one line rather than as a block.
/// The style sits on the [`Line`], not on its spans, so the fill runs to the
/// full width of the row instead of stopping at the end of the text.
fn with_entry_backdrop(line: Line<'static>, entry: &EventEntry<'_>) -> Line<'static> {
    let background = if entry.selected {
        Some(theme::SELECTION_BACKGROUND)
    } else if matches!(entry.event, AgentEvent::UserMessage { .. }) {
        Some(theme::USER_MESSAGE_BACKGROUND)
    } else if is_conversation_change(entry.event) && branch_border(entry.event).is_none() {
        Some(theme::CONVERSATION_CHANGE_BACKGROUND)
    } else {
        None
    };
    match background {
        Some(background) => {
            let style = line.style.bg(background);
            line.style(style)
        }
        None => line,
    }
}

/// The thin border around a branch marker, in its direction's color.
fn branch_border(event: &AgentEvent) -> Option<Color> {
    match event {
        AgentEvent::Branched { direction, .. } => Some(match direction {
            styra_protocol::event::BranchDirection::To => theme::BRANCH_TO,
            styra_protocol::event::BranchDirection::From => theme::BRANCH_FROM,
        }),
        _ => None,
    }
}

fn finished_branch(entry: &EventEntry<'_>) -> bool {
    matches!(entry.event, AgentEvent::Branched { .. })
        && entry.branch.state.as_ref().is_some_and(|state| {
            state.completion.is_done()
                || state.status
                    == crate::interactions::InteractionStatus::Stopped(StopTone::Integrated)
        })
}

/// A conversation already starts with a direction glyph, so tint that glyph
/// rather than inserting another marker. Other events reserve the same first
/// column for a small red dot when selected.
fn selected_summary_line(
    mut line: Line<'static>,
    is_conversation: bool,
    selected: bool,
) -> Line<'static> {
    if !selected {
        return line;
    }
    if is_conversation {
        if let Some(glyph) = line.spans.get_mut(1) {
            glyph.style = glyph.style.fg(theme::SELECTED_ENTRY_MARKER);
        }
    } else if let Some(lead) = line.spans.get_mut(0) {
        *lead = Span::styled("• ", Style::default().fg(theme::SELECTED_ENTRY_MARKER));
    }
    line
}

/// Clip one logical line to `width` columns, marking the cut with `…`. When
/// the line carries a trailing fold marker it is kept at the right edge, so a
/// clipped row still shows that there is more to expand into.
fn truncate_line(mut line: Line<'static>, width: usize, has_marker: bool) -> Line<'static> {
    if line.width() <= width {
        return line;
    }
    let marker = if has_marker && line.spans.len() > 1 {
        line.spans.pop()
    } else {
        None
    };
    // Even a very narrow pane must fit both the cut text and its marker.
    let marker = text::clip(
        Line::from(marker.into_iter().collect::<Vec<_>>()),
        width.saturating_sub(1),
    );
    let mut clipped = text::truncate(
        line,
        width.saturating_sub(marker.width()),
        Some(Style::default().fg(theme::ADDITIONAL_INFO)),
    );
    clipped.spans.extend(marker.spans);
    clipped
}

/// Wrap a rendered line at the pane edge, aligning its continuation rows
/// under the structure the line already has — a list item's text or a table
/// row's border — and falling back to `continuation_indent` for flowing prose.
/// See [`structural_indent`].
pub fn wrap_rendered(
    line: Line<'static>,
    width: usize,
    continuation_indent: usize,
) -> Vec<Line<'static>> {
    // Changed diff rows use the whole pane, including on continuation rows.
    // Their signs can otherwise be mistaken for Markdown list markers.
    if line.spans.iter().any(|span| {
        span.style.bg.is_some_and(|bg| {
            [theme::DIFF_ADDED_BACKGROUND, theme::DIFF_REMOVED_BACKGROUND].contains(&bg)
        })
    }) {
        return wrap_line(line, width, 0);
    }
    let indent = structural_indent(&line).unwrap_or(continuation_indent);
    wrap_line(line, width, indent)
}

/// Wrap all of an expanded log entry, keeping rendered Markdown tables as a
/// unit. A table wider than the pane gives space back from its widest column
/// first and wraps that column's cells inside the table. Sending each row
/// through [`wrap_rendered`] independently would instead break borders at
/// arbitrary words and make the columns stop lining up.
fn wrap_log_lines(
    lines: Vec<Line<'static>>,
    width: usize,
    summary_indent: usize,
) -> Vec<Line<'static>> {
    let mut wrapped = Vec::new();
    let mut lines = lines.into_iter();
    let mut index = 0usize;

    while let Some(line) = lines.next() {
        if is_table_border(&line, '┌') {
            let mut table = vec![line];
            for next in lines.by_ref() {
                let is_bottom = is_table_border(&next, '└');
                table.push(next);
                if is_bottom {
                    break;
                }
            }
            if let Some(resized) = resize_rendered_table(&table, width) {
                wrapped.extend(resized);
                index += table.len();
                continue;
            }
            for line in table {
                wrapped.extend(wrap_rendered(line, width, DETAIL_INDENT.len()));
                index += 1;
            }
            continue;
        }

        let continuation_indent = if index == 0 {
            summary_indent
        } else {
            DETAIL_INDENT.len()
        };
        wrapped.extend(wrap_rendered(line, width, continuation_indent));
        index += 1;
    }
    wrapped
}

fn is_table_border(line: &Line<'_>, border: char) -> bool {
    line.spans
        .iter()
        .flat_map(|span| span.content.chars())
        .find(|ch| !ch.is_whitespace())
        == Some(border)
}

/// Resize a complete `tui-markdown` table to `width` and preserve its styles.
/// Returns `None` for an unfamiliar table shape so the normal safe wrapper can
/// still handle it.
fn resize_rendered_table(table: &[Line<'static>], width: usize) -> Option<Vec<Line<'static>>> {
    let top = table.first()?;
    if top.width() <= width {
        return Some(table.to_vec());
    }
    let top_text = line_text(top);
    let trimmed = top_text.trim_start();
    let prefix_width = text::width(&top_text[..top_text.len() - trimmed.len()]);
    let inside = trimmed.strip_prefix('┌')?.strip_suffix('┐')?;
    let mut column_widths: Vec<usize> = inside
        .split('┬')
        .map(|segment| text::width(segment).checked_sub(2))
        .collect::<Option<_>>()?;
    if column_widths.is_empty() {
        return None;
    }

    let fixed_width = prefix_width + column_widths.len() * 3 + 1;
    let available_content = width.checked_sub(fixed_width)?;
    if available_content < column_widths.len() {
        return None;
    }
    while column_widths.iter().sum::<usize>() > available_content {
        let (widest, &widest_width) = column_widths
            .iter()
            .enumerate()
            .max_by_key(|&(index, column_width)| (*column_width, std::cmp::Reverse(index)))?;
        if widest_width <= 1 {
            return None;
        }
        column_widths[widest] -= 1;
    }

    let mut resized = Vec::new();
    for line in table {
        let text = line_text(line);
        let first = text.trim_start().chars().next()?;
        match first {
            '┌' => resized.push(resized_border(line, &column_widths, '┌', '┬', '┐')?),
            '├' => resized.push(resized_border(line, &column_widths, '├', '┼', '┤')?),
            '└' => resized.push(resized_border(line, &column_widths, '└', '┴', '┘')?),
            '│' => resized.extend(resized_row(line, &column_widths)?),
            _ => return None,
        }
    }
    Some(resized)
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn resized_border(
    line: &Line<'static>,
    widths: &[usize],
    left: char,
    intersection: char,
    right: char,
) -> Option<Line<'static>> {
    let border_index = line
        .spans
        .iter()
        .position(|span| span.content.contains(left))?;
    let mut spans = line.spans[..border_index].to_vec();
    let style = line.spans[border_index].style;
    let mut border = String::new();
    border.push(left);
    for (index, width) in widths.iter().enumerate() {
        border.push_str(&"─".repeat(width + 2));
        border.push(if index + 1 == widths.len() {
            right
        } else {
            intersection
        });
    }
    spans.push(Span::styled(border, style));
    Some(Line::from(spans).style(line.style))
}

fn resized_row(line: &Line<'static>, widths: &[usize]) -> Option<Vec<Line<'static>>> {
    let first_border = line
        .spans
        .iter()
        .position(|span| span.content.as_ref() == "│")?;
    let prefix = line.spans[..first_border].to_vec();
    let border_style = line.spans[first_border].style;
    let mut cells = Vec::new();
    let mut padding_styles = Vec::new();
    let mut cell = Vec::new();
    for span in &line.spans[first_border + 1..] {
        if span.content.as_ref() == "│" {
            padding_styles.push(cell_padding_style(&cell));
            cells.push(trim_cell(std::mem::take(&mut cell)));
        } else {
            cell.push(span.clone());
        }
    }
    if cells.len() != widths.len() {
        return None;
    }

    let wrapped_cells: Vec<Vec<Line<'static>>> = cells
        .into_iter()
        .zip(widths)
        .map(|(cell, &width)| wrap_line(Line::from(cell), width, 0))
        .collect();
    let height = wrapped_cells.iter().map(Vec::len).max().unwrap_or(1);
    let mut rows = Vec::with_capacity(height);
    for row_index in 0..height {
        let mut spans = prefix.clone();
        spans.push(Span::styled("│", border_style));
        for (column, &width) in widths.iter().enumerate() {
            let content = wrapped_cells[column].get(row_index);
            let content_width = content.map(Line::width).unwrap_or(0);
            let padding_style = padding_styles[column];
            spans.push(Span::styled(" ", padding_style));
            if let Some(content) = content {
                spans.extend(content.spans.clone());
            }
            spans.push(Span::styled(
                " ".repeat(width.saturating_sub(content_width) + 1),
                padding_style,
            ));
            spans.push(Span::styled("│", border_style));
        }
        rows.push(Line::from(spans).style(line.style));
    }
    Some(rows)
}

/// The style a resized cell's padding is drawn in: that of the whitespace
/// `tui-markdown` padded the cell with. Borrowing the content's style instead
/// would spread a code or entry background from the cell's first word across
/// its padding and every blank row beneath it.
fn cell_padding_style(cell: &[Span<'static>]) -> Style {
    cell.iter()
        .find(|span| !span.content.is_empty() && span.content.trim().is_empty())
        .map(|span| span.style)
        .unwrap_or_default()
}

fn trim_cell(mut spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    while spans
        .first()
        .is_some_and(|span| span.content.trim().is_empty())
    {
        spans.remove(0);
    }
    while spans
        .last()
        .is_some_and(|span| span.content.trim().is_empty())
    {
        spans.pop();
    }
    if let Some(first) = spans.first_mut() {
        first.content = first.content.trim_start().to_owned().into();
    }
    if let Some(last) = spans.last_mut() {
        last.content = last.content.trim_end().to_owned().into();
    }
    spans
}

fn is_conversation_change(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::Branched { .. }
            | AgentEvent::ProviderChanged { .. }
            | AgentEvent::ModelChanged { .. }
    )
}

/// Render an entry's header for its display context. Expansion state and
/// detail availability come from the entry itself.
pub fn summary_line(
    entry: &EventEntry<'_>,
    context: SummaryContext,
    protocol: Protocol,
) -> Line<'static> {
    let tag = entry.event.tag();
    let conversation_change = is_conversation_change(entry.event);
    let branch_appearance = match entry.event {
        AgentEvent::Branched { direction, .. } => Some(match direction {
            styra_protocol::event::BranchDirection::To => theme::BRANCH_TO,
            styra_protocol::event::BranchDirection::From => theme::BRANCH_FROM,
        }),
        _ => None,
    };
    let change_color = branch_appearance.unwrap_or(theme::CONVERSATION_CHANGE);
    let is_conversation = matches!(
        entry.event,
        AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
    );
    // A branch marker leads with a bar down its band's edge rather than the
    // diamond the other conversation changes share.
    let row_lead = if is_conversation {
        ""
    } else if branch_appearance.is_some() {
        "▌ "
    } else if conversation_change {
        "◆ "
    } else {
        "  "
    };
    let display_tag = match entry.event {
        AgentEvent::UserMessage { .. } => "»",
        AgentEvent::AgentMessage { .. } => "«",
        AgentEvent::CommandStarted { .. } | AgentEvent::CommandCompleted { .. } => "Shell",
        AgentEvent::ToolStarted { name, .. } | AgentEvent::ToolCompleted { name, .. }
            if name == "Bash" =>
        {
            "Shell"
        }
        AgentEvent::ToolStarted { name, .. } | AgentEvent::ToolCompleted { name, .. } => {
            name.as_str()
        }
        _ => tag,
    };
    // Shell rows use one color across their whole summary, matching the old
    // Codex command presentation. A running shell has no result marker; its
    // completed replacement gains a checkmark/cross like every other tool.
    // Some providers only report the final shell expression's status. An
    // unguarded pipeline can therefore return zero while an earlier command
    // printed a clear failure; keep that distinct from both success and a
    // provider-reported failure with an amber warning.
    let (summary_style, prefix, prefix_style) = match entry.event {
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" && failed_shell_result(entry.event) =>
        {
            (
                Style::default().fg(theme::ERROR),
                "✗ ",
                Style::default().fg(theme::ERROR),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" && suspicious_shell_success(entry.event) =>
        {
            (
                Style::default().fg(theme::WARNING),
                "⚠ ",
                Style::default().fg(theme::WARNING),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" =>
        {
            (
                Style::default().fg(tag_color(tag)),
                "✓ ",
                Style::default().fg(theme::SUCCESS),
            )
        }
        _ if tag == "shell" => (Style::default().fg(tag_color(tag)), "", Style::default()),
        AgentEvent::ToolCompleted { status, .. } | AgentEvent::CommandCompleted { status, .. }
            if matches!(status.as_str(), "error" | "failed") =>
        {
            (
                Style::default().fg(theme::ERROR),
                "✗ ",
                Style::default().fg(theme::ERROR),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. } => (
            Style::default().fg(theme::TEXT),
            "✓ ",
            Style::default().fg(theme::SUCCESS),
        ),
        _ if conversation_change => (
            Style::default()
                .fg(change_color)
                .add_modifier(Modifier::BOLD),
            "",
            Style::default(),
        ),
        _ => (
            Style::default().fg(message_text_color(tag)),
            "",
            Style::default(),
        ),
    };
    // The cursor's text in the soft cyan the live list draws its cursor's in,
    // so the two lists mark their selection alike. The tag and the result
    // mark keep their own colors, so what the entry is still reads.
    let severity_colored = matches!(tag, "error" | "malformed")
        || failed_shell_result(entry.event)
        || suspicious_shell_success(entry.event)
        || matches!(entry.event, AgentEvent::ToolCompleted { status, .. } | AgentEvent::CommandCompleted { status, .. } if matches!(status.as_str(), "error" | "failed"));
    let summary_style = if entry.selected && !severity_colored {
        summary_style.fg(theme::SELECTED_INTERACTION_TEXT)
    } else {
        summary_style
    };
    let mut spans = vec![Span::styled(row_lead, Style::default().fg(change_color))];
    if branch_appearance.is_some() {
        if matches!(
            entry.event,
            AgentEvent::Branched {
                direction: styra_protocol::event::BranchDirection::To,
                ..
            }
        ) {
            if let Some(state) = &entry.branch.state {
                spans.extend([state.block(), Span::raw(" ")]);
            }
        }
    } else {
        spans.push(Span::styled(
            if is_conversation {
                format!("{display_tag} ")
            } else {
                format!("{display_tag:<8}")
            },
            Style::default()
                .fg(tag_color(tag))
                .add_modifier(Modifier::BOLD),
        ));
    }
    if context == SummaryContext::EventList {
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix, prefix_style));
        }
        let mut summary = file_action_summary(entry.event)
            .unwrap_or_else(|| protocol.presented_summary(entry.event, PresentationMode::Pretty));
        if matches!(entry.event, AgentEvent::AgentMessage { text } if crate::answer::sections(text).next().is_some())
        {
            if let Some(DetailBlock::Text(text)) =
                crate::answer::presented_detail(entry.event, protocol).first()
            {
                summary = text.lines().next().unwrap_or_default().to_owned();
            }
        }
        if entry.expanded && summary.ends_with('…') {
            if let Some(first_line) = crate::answer::presented_detail(entry.event, protocol)
                .first()
                .and_then(|block| match block {
                    DetailBlock::Text(text) | DetailBlock::Code { text, .. } => text.lines().next(),
                })
            {
                summary = first_line.to_owned();
            }
        }
        // The marker reads by where it leads, under the name the operator
        // knows that Session by, drawn as the navigator draws its row: the
        // status block, the name, then how it came to be in that state.
        let mut branch_tail = Vec::new();
        if let AgentEvent::Branched { direction, .. } = entry.event {
            summary = match direction {
                styra_protocol::event::BranchDirection::To => String::new(),
                styra_protocol::event::BranchDirection::From => "from:".to_owned(),
            };
            if !summary.is_empty() {
                branch_tail.push(Span::raw(" "));
            }
            branch_tail.push(Span::styled(
                entry.branch.name.unwrap_or("untitled").to_owned(),
                summary_style,
            ));
            if let Some((from, to)) = entry.branch_provider_switch {
                branch_tail.push(Span::styled(
                    format!(" (provider switch: {} → {})", from.as_str(), to.as_str()),
                    summary_style,
                ));
            }
            if !entry.branch.tags.is_empty() {
                branch_tail.push(Span::styled(
                    format!("  #{}", entry.branch.tags.join(" #")),
                    Style::default().fg(theme::INTERACTION_TAG),
                ));
            }
            // Only a worktree other than this one is named: most branches
            // share their source's checkout, and saying so on each is noise.
            if let Some(worktree) = entry.branch.worktree {
                branch_tail.push(Span::styled(
                    format!("  ⎇ {worktree}"),
                    Style::default().fg(theme::ACCENT),
                ));
            }
            if let Some(state) = &entry.branch.state {
                branch_tail.extend(state.labels());
            }
        }
        let display_summary = match entry.event {
            AgentEvent::ToolStarted { name, .. } | AgentEvent::ToolCompleted { name, .. } => {
                summary
                    .strip_prefix(name)
                    .unwrap_or(&summary)
                    .trim_start_matches(": ")
            }
            _ => &summary,
        };
        spans.extend(parse_inline_spans_with_highlight(
            display_summary,
            summary_style,
            entry.link_highlight,
        ));
        spans.extend(branch_tail);
    }
    // The framing this turn was sent with is stripped from the message, so the
    // row says what was asked of it instead of showing ten lines saying so.
    if let Some(contract) = entry.contract {
        spans.push(Span::styled(
            format!(" ⟨{}⟩", contract.as_str()),
            Style::default().fg(theme::ACCENT),
        ));
    }
    if context == SummaryContext::EventList && entry.has_detail {
        let marker = if entry.expanded { " " } else { "▸" };
        spans.push(Span::styled(
            format!(" {marker}"),
            Style::default().fg(theme::ADDITIONAL_INFO),
        ));
    }
    Line::from(spans)
}

fn failed_shell_result(event: &AgentEvent) -> bool {
    match event {
        AgentEvent::ToolCompleted { status, .. } => {
            matches!(status.as_str(), "error" | "failed")
        }
        AgentEvent::CommandCompleted {
            status, exit_code, ..
        } => {
            matches!(status.as_str(), "error" | "failed") || exit_code.is_some_and(|code| code != 0)
        }
        _ => false,
    }
}

/// A successful shell result whose own output strongly suggests that a nested
/// command failed. This is deliberately conservative: arbitrary mentions of
/// "error" (test names, grep results, documentation) remain green.
pub fn suspicious_shell_success(event: &AgentEvent) -> bool {
    if failed_shell_result(event) {
        return false;
    }
    let output = match event {
        AgentEvent::ToolCompleted { name, output, .. } if name == "Bash" => output,
        AgentEvent::CommandCompleted { output, .. } => output,
        _ => return false,
    };
    output.lines().any(is_error_diagnostic)
}

/// File-event summaries should say what happened, not merely repeat paths
/// under an opaque `files` tag. Providers do not always report a change kind,
/// so unified-diff creation/deletion markers are used when present and the
/// honest fallback is "changed".
fn file_action_summary(event: &AgentEvent) -> Option<String> {
    let (paths, diff) = match event {
        AgentEvent::FileChanged { paths, diff, .. } => (paths.clone(), diff.as_deref()),
        AgentEvent::DiffUpdated { diff } => {
            let paths = event
                .summary()
                .split(", ")
                .map(str::to_owned)
                .collect::<Vec<_>>();
            (paths, Some(diff.as_str()))
        }
        _ => return None,
    };
    let action = match diff {
        Some(diff) => {
            let added =
                diff.contains("new file mode") || diff.lines().any(|line| line == "--- /dev/null");
            let deleted = diff.contains("deleted file mode")
                || diff.lines().any(|line| line == "+++ /dev/null");
            match (added, deleted) {
                (true, false) => "added",
                (false, true) => "deleted",
                (false, false) => "modified",
                (true, true) => "changed",
            }
        }
        None => "changed",
    };
    Some(format!("{action} {}", paths.join(", ")))
}

/// Drop the name an [`AgentEvent::Branched`] detail block carries: it is a
/// snapshot taken when the marker was written, and the marker's summary
/// already reads by the live one. A no-op for every other event.
pub(crate) fn without_branch_snapshot(
    event: &AgentEvent,
    blocks: Vec<DetailBlock>,
) -> Vec<DetailBlock> {
    if !matches!(event, AgentEvent::Branched { .. }) {
        return blocks;
    }
    blocks
        .into_iter()
        .map(|block| match block {
            DetailBlock::Text(text) => DetailBlock::Text(
                text.lines()
                    .filter(|line| !line.starts_with("name: "))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            other => other,
        })
        .collect()
}

/// The other session's last message, quoted on one line as the navigator
/// quotes it beneath that session's row. A view cuts it to its width rather
/// than wrapping it.
pub(crate) fn branch_message_line(branch: &BranchLink<'_>) -> Option<Line<'static>> {
    branch.message.map(|text| {
        Line::from(Span::styled(
            format!(
                "{DETAIL_INDENT}{}",
                crate::interactions::last_message_text(text)
            ),
            Style::default().fg(crate::interactions::LAST_MESSAGE_COLOR),
        ))
    })
}

/// The pretty, provider-aware expandable body of an entry. The preview panel
/// owns the optional raw view.
pub fn detail_lines_with_links(
    event: &AgentEvent,
    protocol: Protocol,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text_color = message_text_color(event.tag());
    let suspicious_shell = suspicious_shell_success(event);
    let mut entries_before = 0;
    for block in without_branch_snapshot(event, crate::answer::presented_detail(event, protocol)) {
        match block {
            DetailBlock::Text(text) => {
                let base_style = Style::default().fg(text_color);
                let rendered = markdown_block_render(
                    &text,
                    base_style,
                    DETAIL_INDENT,
                    links,
                    highlight.and_then(|index| index.checked_sub(entries_before)),
                );
                entries_before += rendered.entries;
                lines.extend(rendered.lines);
            }
            DetailBlock::Code { text, language } => {
                lines.extend(code_block_lines(
                    &text,
                    language.as_deref(),
                    text_color,
                    suspicious_shell,
                    DETAIL_INDENT,
                ));
            }
        }
    }
    lines
}

/// `running/idle/stopped`, each number in its own colour and nothing else:
/// the counts are the label.
fn activity_spans(counts: ActivityCounts) -> Vec<Span<'static>> {
    if counts == ActivityCounts::default() {
        return Vec::new();
    }
    let separator = || Span::styled("/", Style::default().fg(theme::MUTED_TEXT));
    let count =
        |value: usize, color: Color| Span::styled(value.to_string(), Style::default().fg(color));
    vec![
        Span::raw(" "),
        count(counts.running, theme::WARNING),
        separator(),
        count(counts.idle, theme::SUCCESS),
        separator(),
        count(counts.stopped, theme::MUTED_TEXT),
        Span::raw(" "),
    ]
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{}h{:02}m", seconds / 3600, (seconds % 3600) / 60)
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format_scaled(tokens, 1_000, 'k')
    } else {
        format_scaled(tokens, 1_000_000, 'M')
    }
}

fn format_scaled(tokens: u64, unit: u64, suffix: char) -> String {
    let whole = tokens / unit;
    if whole < 10 {
        format!("{whole}.{}{suffix}", (tokens % unit) * 10 / unit)
    } else {
        format!("{whole}{suffix}")
    }
}

#[cfg(test)]
mod tests;
