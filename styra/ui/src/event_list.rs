//! The main event list: each entry a summary line that grows inline when
//! expanded, plus the empty-list start screen and the trailing status tail.

use crate::chrome::{all_events_title, panel_block, PanelChrome};
use crate::code::{code_block_lines, is_error_diagnostic};
use crate::footer::{message_text_color, tag_color};
use crate::markdown::{
    markdown_block_render, parse_inline_spans_with_highlight, structural_indent, EntryIndex,
    LinkDisplay,
};
use crate::palette;
use crate::render_cache::{Memo, Weigh};
use crate::search::{self, SearchView};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::cell::RefCell;
use std::time::Duration;
use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};
use styra_protocol::Contract;

const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";

/// Which row this is, and which state of it — see the host's event identity
/// (`tui::timeline::EventVersion`), which this mirrors.
///
/// A row is not identified by its position: the filters renumber those every
/// time they change. Nor by its event alone: a command completing or a task
/// reporting rewrites a row already on the list rather than appending a new
/// one. The pair is what stays true, and it is what [`entry_item`] keys its
/// cached rendering on.
///
/// Opaque here on purpose. This crate does not know how the host mints these
/// and only ever compares them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EntryVersion {
    pub id: u64,
    pub revision: u32,
}

pub struct EventEntry<'a> {
    pub event: &'a AgentEvent,
    /// Which row, and which state of it. See [`EntryVersion`].
    pub version: EntryVersion,
    pub expanded: bool,
    pub has_detail: bool,
    pub contract: Option<&'a Contract>,
    pub selected: bool,
    /// The Markdown link within this entry to mark, if link navigation is on it.
    pub link_highlight: Option<EntryIndex>,
}

pub enum EventListStatus {
    Pending,
    Running {
        elapsed: Duration,
        quiet: Option<Duration>,
        events: usize,
    },
    /// `reason` says what left the session idle, where saying it adds
    /// something: an interrupted turn and a finished one leave the same
    /// screen otherwise.
    Idle {
        reason: Option<String>,
    },
    Background {
        elapsed: Duration,
    },
    Stopped {
        elapsed: Duration,
        reason: Option<String>,
        tone: crate::chrome::StopTone,
    },
    Ended,
}

/// Listed Interactions by activity, shown as a tight `running/idle/stopped`
/// tally: one glance at the fleet without opening the navigator.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ActivityCounts {
    pub running: usize,
    pub idle: usize,
    pub stopped: usize,
    /// Interactions that went idle unseen and have not been focused since,
    /// announced beside the tally until they are.
    pub newly_idle: usize,
}

pub struct EventListView<'a> {
    pub chrome: PanelChrome,
    pub entries: Vec<EventEntry<'a>>,
    /// The whole fleet's tally, on this pane's bottom border — see
    /// [`ActivityCounts`].
    pub activity: ActivityCounts,
    /// The list shows every event rather than the default conversation only,
    /// which the bottom border then says, along with `show_minor`.
    pub all_events: bool,
    /// Whether minor lifecycle events are among them; see [`all_events_title`].
    pub show_minor: bool,
    /// The interaction being shown has stopped working and left uncommitted
    /// changes in its repository — see [`crate::chrome::uncommitted_title`].
    pub uncommitted_changes: bool,
    pub usage: Option<(u64, u64, u64)>,
    pub can_configure_launch: bool,
    pub selection_name: String,
    pub requested_offset: usize,
    /// Whether an explicit move toward older entries permits scrolloff to
    /// reveal rows above the current viewport anchor. Live content updates do
    /// not set this: a row changing height must not look like navigation.
    pub moved_backward: bool,
    pub protocol: Protocol,
    pub links: LinkDisplay,
    /// The `/` search: what has been typed, and whether the prompt still has
    /// the keys. See [`crate::search`].
    pub search: SearchView<'a>,
    pub status: EventListStatus,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventListFeedback {
    pub effective_offset: usize,
}

pub struct EntryLogView<'a> {
    pub entries: Vec<EventEntry<'a>>,
    /// Whether this pane, rather than the event list above it, is holding the
    /// navigation keys. A focused pane wears the active border and names the
    /// key that hands them back; only then does any of its rows carry the
    /// selection, so the screen never shows two cursors at once.
    pub focused: bool,
    pub requested_scroll: u16,
    pub protocol: Protocol,
    pub links: LinkDisplay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryLogFeedback {
    pub limit: u16,
    pub effective_scroll: u16,
}

pub fn render_entry_log(
    frame: &mut Frame,
    view: &EntryLogView<'_>,
    area: Rect,
) -> EntryLogFeedback {
    use ratatui::widgets::{Block, Borders};
    let (border, title) = if view.focused {
        (
            palette::ACCENT,
            " entry log · Tab: back to the list · E: close ",
        )
    } else {
        (
            palette::INACTIVE,
            " entry log · follows selection · Tab: read it · E: close ",
        )
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border))
        .title(Span::styled(
            title,
            Style::default().fg(palette::MUTED_TEXT),
        ));
    if view.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  nothing in the interaction log yet",
                Style::default().fg(palette::MUTED_TEXT),
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
            Style::default().fg(palette::MUTED_TEXT),
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
    let items = view
        .entries
        .iter()
        .map(|entry| entry_item(entry, width, viewport, entry_render));
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
    let mut block = panel_block(&view.chrome).title_bottom(Line::from(usage).right_aligned());
    let activity = activity_spans(view.activity);
    if !activity.is_empty() {
        block = block.title_bottom(Line::from(activity));
    }
    if view.all_events {
        block = block.title_bottom(all_events_title(view.show_minor).right_aligned());
    }
    if view.uncommitted_changes {
        block = crate::chrome::uncommitted_title(block);
    }
    if let Some(search) = search_title(&view.search) {
        block = block.title_bottom(search);
    }

    if view.entries.is_empty() {
        // Before anything is launched, the empty list is the start screen: the
        // one moment the agent, model, and effort are still open, so it says
        // what they are and how to change them instead of only waiting.
        let lines = if view.can_configure_launch {
            vec![
                Line::from(vec![
                    Span::styled(
                        "  launching with ",
                        Style::default().fg(palette::MUTED_TEXT),
                    ),
                    Span::styled(
                        view.selection_name.as_str(),
                        Style::default()
                            .fg(palette::ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    "  press L to choose the default agent, model, and effort — or i to write the first message",
                    Style::default().fg(palette::MUTED_TEXT),
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "  waiting for the agent — press i to send a message",
                Style::default().fg(palette::MUTED_TEXT),
            ))]
        };
        frame.render_widget(Paragraph::new(lines).block(block), area);
        return EventListFeedback::default();
    }

    let width = area.width.saturating_sub(2) as usize;
    let viewport_height = area.height.saturating_sub(2) as usize;
    let entry_render = EntryRender {
        protocol: view.protocol,
        links: view.links,
        search: view.search.term(),
    };
    // Built on demand: the offset math below asks about a window of items, not
    // all of them. See [`LazyItems`] and [`Heights`].
    //
    // The status tail is one of them. Including it is what lets the scroll
    // decision see the row waiting below the last entry — otherwise moving
    // past a tall entry can look attractive merely because the algorithm
    // cannot see what is under it.
    let mut items = LazyItems {
        entries: &view.entries,
        tail: ListItem::new(status_tail(&view.status)),
        built: (0..=view.entries.len()).map(|_| None).collect(),
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
    let position = view.entries.iter().position(|entry| entry.selected);
    let offset = list_offset_with_scrolloff(
        view.requested_offset,
        position,
        &mut items,
        viewport_height,
        view.moved_backward,
    );
    // Only what the viewport can show is built, and only that is handed over.
    // A session of any length therefore costs one screen of rendering per
    // frame rather than its whole history — see [`LazyItems::into_window`].
    let mut items = items.into_window(offset, position);
    clip_boundary_entry(
        &mut items,
        &view.entries,
        offset,
        viewport_height,
        width,
        entry_render,
    );
    let list = List::new(items).block(block);
    // Both indices are rebased onto the window, which begins at `offset`: to
    // ratatui this is the whole list, seen from the top.
    //
    // `ListState::select(None)` also resets the offset to zero, and this list
    // renders with nothing selected whenever the selected entry is one the
    // filters hide. Assign the field directly so that a computed offset is
    // never thrown away: the offset this render reports back is persisted, so
    // a zero here would scroll the interaction log to the top and keep it
    // there rather than flickering for one frame.
    *state.selected_mut() = position.map(|position| position - offset);
    *state.offset_mut() = 0;
    frame.render_stateful_widget(list, area, &mut state);
    EventListFeedback {
        // Back into the caller's numbering. Ratatui only ever moves the offset
        // forward from where it was put, so this stays within the window.
        effective_offset: offset + state.offset(),
    }
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

        let entry = &entries[item_index];
        let is_last_entry = item_index + 1 == entries.len();
        let max_rows = if is_last_entry && remaining > 1 {
            remaining - 1
        } else {
            remaining
        };
        items[position] = entry_item_with_max_rows(entry, width, max_rows, entry_render);
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

impl LazyItems<'_> {
    fn item(&mut self, index: usize) -> &ListItem<'static> {
        self.built[index].get_or_insert_with(|| match self.entries.get(index) {
            Some(entry) => entry_item(entry, self.width, self.viewport_height, self.render),
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
    fn into_window(mut self, start: usize, selected: Option<usize>) -> Vec<ListItem<'static>> {
        let mut used = 0usize;
        let mut end = start;
        while end < Heights::len(&self) {
            used = used.saturating_add(self.height(end));
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

fn status_tail(status: &EventListStatus) -> Line<'static> {
    let (text, color) = match status {
        EventListStatus::Pending => (
            "  … waiting for your first message".to_string(),
            palette::INACTIVE,
        ),
        EventListStatus::Running {
            elapsed,
            quiet,
            events,
        } => (running_tail(*elapsed, *quiet, *events), palette::RUNNING),
        // Idle carries no elapsed figure: nothing is happening, so a
        // climbing counter only draws the eye to a number that means nothing.
        EventListStatus::Idle { reason } => {
            (format!("  ── idle{} ──", why(reason)), palette::SUCCESS)
        }
        EventListStatus::Background { elapsed } => (
            format!(
                "  ── idle {} · background work still running ──",
                format_duration(*elapsed)
            ),
            palette::WARNING,
        ),
        EventListStatus::Stopped { reason, tone, .. } => {
            (format!("  ── stopped{} ──", why(reason)), tone.color())
        }
        _ => return Line::default(),
    };
    Line::from(Span::styled(text, Style::default().fg(color)))
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
    search: Option<String>,
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
        search: render.search.map(str::to_owned),
    };
    let rows = ROW_CACHE.with(|cache| {
        cache
            .borrow_mut()
            .get_or_insert_with(key, || build_entry_rows(entry, width, max_rows, render))
    });
    ListItem::new(rows)
}

/// [`entry_item_with_max_rows`] proper, behind its cache: every finished row
/// of one entry.
fn build_entry_rows(
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
        summary_line(entry, entry.expanded, entry.has_detail, true, protocol),
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
    let mut detail =
        detail_lines_with_links(entry.event, protocol, None, links, entry.link_highlight);
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
                Style::default().fg(palette::WARNING),
            )),
        );
    }
    if detail.len() > MAX_DETAIL_LINES {
        let hidden = detail.len() - MAX_DETAIL_LINES;
        detail.truncate(MAX_DETAIL_LINES);
        detail.push(Line::from(Span::styled(
            format!("{DETAIL_INDENT}… {hidden} more lines"),
            Style::default().fg(palette::MUTED_TEXT),
        )));
    }
    lines.extend(detail);
    let mut wrapped = wrap_log_lines(lines, width, summary_indent);
    // The cap above bounds logical detail lines, which say nothing about how
    // many rows they occupy once wrapped, so the height has to be bounded
    // again here.
    if wrapped.len() > max_rows {
        if max_rows == 1 {
            wrapped.truncate(1);
            if let Some(summary) = wrapped.first_mut() {
                summary
                    .spans
                    .push(Span::styled(" …", Style::default().fg(palette::MUTED_TEXT)));
            }
        } else {
            let hidden = wrapped.len() - (max_rows - 1);
            wrapped.truncate(max_rows - 1);
            wrapped.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}… {hidden} more rows — press p for the full entry"),
                Style::default().fg(palette::MUTED_TEXT),
            )));
        }
    }
    // After wrapping, so a word broken across two rows is marked on the row it
    // is actually on rather than half-marked on both. The mark is a span
    // style and the selection below is the row's own, so they coexist.
    let mut wrapped = search::highlight_lines(wrapped, search);
    if let Some(first) = wrapped.first_mut() {
        *first = with_entry_backdrop(std::mem::take(first), entry);
    }
    wrapped
}

/// Tint operator messages, and mark a selected row, by backing its first row
/// only. An expanded entry's detail body keeps the plain background, so either
/// cue reads as one line rather than as a block.
/// The style sits on the [`Line`], not on its spans, so the fill runs to the
/// full width of the row instead of stopping at the end of the text.
fn with_entry_backdrop(line: Line<'static>, entry: &EventEntry<'_>) -> Line<'static> {
    let background = if entry.selected {
        Some(palette::SELECTION_BACKGROUND)
    } else if matches!(entry.event, AgentEvent::UserMessage { .. }) {
        Some(palette::USER_MESSAGE_BACKGROUND)
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

/// A conversation already starts with a direction glyph, so tint that glyph
/// rather than inserting another marker. Other events reserve the same first
/// column for a small yellow dot when selected.
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
            glyph.style = glyph.style.fg(palette::SELECTION_MARKER);
        }
    } else if let Some(lead) = line.spans.get_mut(0) {
        *lead = Span::styled("• ", Style::default().fg(palette::SELECTION_MARKER));
    }
    line
}

/// Clip one logical line to `width` columns, marking the cut with `…`. When
/// the line carries a trailing fold marker it is kept at the right edge, so a
/// clipped row still shows that there is more to expand into.
fn truncate_line(line: Line<'static>, width: usize, has_marker: bool) -> Line<'static> {
    if width == 0 {
        return line;
    }
    let mut spans = line.spans;
    let marker = if has_marker && spans.len() > 1 {
        spans.pop()
    } else {
        None
    };
    let marker_width = marker
        .as_ref()
        .map(|span| span.content.chars().count())
        .unwrap_or(0);
    let total: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    if total + marker_width <= width {
        spans.extend(marker);
        return Line::from(spans);
    }

    // Room for the ellipsis and the marker, both of which sit outside the text.
    let budget = width.saturating_sub(marker_width + 1);
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        let span_width = span.content.chars().count();
        if used + span_width <= budget {
            used += span_width;
            kept.push(span);
            continue;
        }
        let take = budget - used;
        if take > 0 {
            let end = span
                .content
                .char_indices()
                .nth(take)
                .map(|(i, _)| i)
                .unwrap_or(span.content.len());
            kept.push(Span::styled(span.content[..end].to_owned(), span.style));
        }
        break;
    }
    kept.push(Span::styled(
        "…",
        Style::default().fg(palette::ADDITIONAL_INFO),
    ));
    kept.extend(marker);
    Line::from(kept)
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
    let prefix_width = top_text.len() - trimmed.len();
    let inside = trimmed.strip_prefix('┌')?.strip_suffix('┐')?;
    let mut column_widths: Vec<usize> = inside
        .split('┬')
        .map(|segment| segment.chars().count().checked_sub(2))
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
    let mut cell = Vec::new();
    for span in &line.spans[first_border + 1..] {
        if span.content.as_ref() == "│" {
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
            let padding_style = content
                .and_then(|line| line.spans.first())
                .map(|span| span.style)
                .or_else(|| {
                    wrapped_cells[column][0]
                        .spans
                        .first()
                        .map(|span| span.style)
                })
                .unwrap_or_default();
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

/// Word-wrap one logical line to `width` columns, preserving each span's
/// style across the break. Continuation rows use a hanging indent so message
/// text stays aligned with the text following its `«`/`»` marker (and detail
/// rows retain their body indent) instead of jumping to the far-left edge.
/// `List` does not wrap on its own, so long lines would otherwise be clipped.
fn wrap_line(line: Line<'static>, width: usize, continuation_indent: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }

    let mut lines = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;
    let continuation_indent = continuation_indent.min(width.saturating_sub(1));

    let start_continuation = |current: &mut Vec<Span<'static>>, current_width: &mut usize| {
        if continuation_indent > 0 {
            current.push(Span::raw(" ".repeat(continuation_indent)));
            *current_width = continuation_indent;
        }
    };

    for span in line.spans {
        let style = span.style;
        for token in split_keep_whitespace(&span.content) {
            let token_width = token.chars().count();

            if token == " " {
                if current_width + token_width > width {
                    if !current.is_empty() {
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                        start_continuation(&mut current, &mut current_width);
                    }
                    continue;
                }
                current.push(Span::styled(token, style));
                current_width += token_width;
                continue;
            }

            // A token that would not fit even on a continuation row of its
            // own has to be hard-split; wrapping it whole would push it past
            // the pane edge, where the widget clips it out of sight.
            if token_width > width.saturating_sub(continuation_indent) {
                let mut remaining = token.as_str();
                while !remaining.is_empty() {
                    if current_width >= width {
                        lines.push(Line::from(std::mem::take(&mut current)));
                        current_width = 0;
                        start_continuation(&mut current, &mut current_width);
                    }
                    let take = width - current_width;
                    let split_at = remaining
                        .char_indices()
                        .nth(take)
                        .map(|(i, _)| i)
                        .unwrap_or(remaining.len());
                    let (chunk, rest) = remaining.split_at(split_at);
                    current.push(Span::styled(chunk.to_owned(), style));
                    current_width += chunk.chars().count();
                    remaining = rest;
                }
                continue;
            }

            if current_width + token_width > width && !current.is_empty() {
                lines.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
                start_continuation(&mut current, &mut current_width);
            }
            current.push(Span::styled(token, style));
            current_width += token_width;
        }
    }

    if !current.is_empty() || lines.is_empty() {
        lines.push(Line::from(current));
    }
    lines
}

/// Split into words and single-space tokens, so a wrap can drop a leading
/// space on the next line without losing the boundary information.
fn split_keep_whitespace(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    for ch in s.chars() {
        if ch == ' ' {
            if !word.is_empty() {
                tokens.push(std::mem::take(&mut word));
            }
            tokens.push(" ".to_owned());
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}

/// `has_detail` is false when the entry has nothing beyond its summary (e.g.
/// a bare `turn started` marker); folding is meaningless there, so no arrow
/// is shown at all rather than one that never does anything when pressed. An
/// expanded entry also shows no arrow: its content is already on screen, so
/// the marker column is reserved for entries that still have something to
/// unfold.
/// `show_summary` is false in previews, whose detail body carries the full
/// content. Inline expanded entries keep the summary in this first row and
/// omit the matching first detail row, so expansion does not make the header
/// appear empty or move its first line down.
pub fn summary_line(
    entry: &EventEntry<'_>,
    expanded: bool,
    has_detail: bool,
    show_summary: bool,
    protocol: Protocol,
) -> Line<'static> {
    let marker = match (has_detail, expanded) {
        (false, _) => " ",
        (true, true) => " ",
        (true, false) => "▸",
    };
    let tag = entry.event.tag();
    let is_conversation = matches!(
        entry.event,
        AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
    );
    let row_lead = if is_conversation { "" } else { "  " };
    let display_tag = match &entry.event {
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
    let (summary_style, prefix, prefix_style) = match &entry.event {
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" && failed_shell_result(&entry.event) =>
        {
            (
                Style::default().fg(tag_color(tag)),
                "✗ ",
                Style::default().fg(palette::ERROR),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" && suspicious_shell_success(&entry.event) =>
        {
            (
                Style::default().fg(tag_color(tag)),
                "⚠ ",
                Style::default().fg(palette::WARNING),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" =>
        {
            (
                Style::default().fg(tag_color(tag)),
                "✓ ",
                Style::default().fg(palette::SUCCESS),
            )
        }
        _ if tag == "shell" => (Style::default().fg(tag_color(tag)), "", Style::default()),
        AgentEvent::ToolCompleted { status, .. } | AgentEvent::CommandCompleted { status, .. }
            if status == "error" =>
        {
            (
                Style::default().fg(palette::ERROR),
                "✗ ",
                Style::default().fg(palette::ERROR),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. } => (
            Style::default().fg(palette::TEXT),
            "✓ ",
            Style::default().fg(palette::SUCCESS),
        ),
        _ => (
            Style::default().fg(message_text_color(tag)),
            "",
            Style::default(),
        ),
    };
    let mut spans = vec![
        Span::raw(row_lead),
        Span::styled(
            if is_conversation {
                format!("{display_tag} ")
            } else {
                format!("{display_tag:<8}")
            },
            Style::default()
                .fg(tag_color(tag))
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if show_summary {
        if !prefix.is_empty() {
            spans.push(Span::styled(prefix, prefix_style));
        }
        let mut summary = file_action_summary(&entry.event)
            .unwrap_or_else(|| protocol.presented_summary(&entry.event, PresentationMode::Pretty));
        if expanded && summary.ends_with('…') {
            if let Some(first_line) = protocol
                .presented_detail(&entry.event, PresentationMode::Pretty)
                .first()
                .and_then(|block| match block {
                    DetailBlock::Text(text) | DetailBlock::Code { text, .. } => text.lines().next(),
                })
            {
                summary = first_line.to_owned();
            }
        }
        let display_summary = match &entry.event {
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
    }
    // The framing this turn was sent with is stripped from the message, so the
    // row says what was asked of it instead of showing ten lines saying so.
    if let Some(contract) = entry.contract {
        spans.push(Span::styled(
            format!(" ⟨{}⟩", contract.as_str()),
            Style::default().fg(palette::ACCENT),
        ));
    }
    if has_detail {
        spans.push(Span::styled(
            format!(" {marker}"),
            Style::default().fg(palette::ADDITIONAL_INFO),
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

/// The pretty, provider-aware expandable body of an entry. `cap` bounds how
/// many lines are shown inline in the list so one noisy command cannot bury
/// the rest of the session. The preview panel owns the optional raw view.
pub fn detail_lines_with_links(
    event: &AgentEvent,
    protocol: Protocol,
    cap: Option<usize>,
    links: LinkDisplay,
    highlight: Option<EntryIndex>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text_color = message_text_color(event.tag());
    let suspicious_shell = suspicious_shell_success(event);
    let mut entries_before = 0;
    for block in protocol.presented_detail(event, PresentationMode::Pretty) {
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
    if let Some(cap) = cap {
        if lines.len() > cap {
            let hidden = lines.len() - cap;
            lines.truncate(cap);
            lines.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}… {hidden} more lines"),
                Style::default().fg(palette::MUTED_TEXT),
            )));
        }
    }
    lines
}

/// The `/` search, shown along the bottom of the list it is marking.
///
/// It says how much is still missing while the term is too short, so a search
/// that highlights nothing yet does not look like a search that found nothing.
fn search_title(search: &SearchView<'_>) -> Option<Line<'static>> {
    let query = search.query?;
    let mut text = format!(" /{query}");
    if search.typing {
        text.push('▌');
    }
    let missing = search::MIN_TERM.saturating_sub(query.trim().chars().count());
    if missing > 0 {
        text.push_str(&format!(
            " · {missing} more character{}",
            if missing == 1 { "" } else { "s" }
        ));
    }
    text.push(' ');
    Some(Line::from(Span::styled(
        text,
        Style::default()
            .fg(palette::ACCENT)
            .add_modifier(Modifier::BOLD),
    )))
}

/// `running/idle/stopped`, each number in its own colour and nothing else:
/// the counts are the label. Newly idle interactions follow, with the key that
/// goes to them.
fn activity_spans(counts: ActivityCounts) -> Vec<Span<'static>> {
    if counts == ActivityCounts::default() {
        return Vec::new();
    }
    let separator = || Span::styled("/", Style::default().fg(palette::MUTED_TEXT));
    let count =
        |value: usize, color: Color| Span::styled(value.to_string(), Style::default().fg(color));
    let mut spans = vec![
        Span::raw(" "),
        count(counts.running, palette::WARNING),
        separator(),
        count(counts.idle, palette::SUCCESS),
        separator(),
        count(counts.stopped, palette::MUTED_TEXT),
        Span::raw(" "),
    ];
    if counts.newly_idle > 0 {
        spans.push(Span::styled(
            format!(
                "^a {} interaction{} idle ",
                counts.newly_idle,
                if counts.newly_idle == 1 { "" } else { "s" }
            ),
            Style::default().fg(palette::SUCCESS),
        ));
    }
    spans
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
mod tests {
    use super::*;
    use super::{format_tokens, list_offset_with_scrolloff, wrap_log_lines};
    use crate::chrome::StatusTone;
    use crate::markdown::markdown_block_lines;
    use ratatui::backend::TestBackend;
    use ratatui::style::Style;
    use ratatui::Terminal;

    /// An identity no other entry in this process has.
    ///
    /// The row cache is keyed on the version and lives in a thread-local, and
    /// the test harness is free to run several tests on one thread. Two tests
    /// that both wrote a literal id would then be asking the same cache the
    /// same question about different events, and the second would be answered
    /// with the first one's rows. Minting these the way a host does keeps each
    /// test's entry its own.
    fn version() -> EntryVersion {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        EntryVersion {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            revision: 0,
        }
    }

    /// Draw a list of `events` with `selected` under the cursor, anchored at
    /// `requested_offset`, and report the screen and the offset the render
    /// asked to keep.
    fn scrolled_screen(
        events: &[AgentEvent],
        selected: usize,
        requested_offset: usize,
    ) -> (Vec<String>, usize) {
        let entries = events
            .iter()
            .enumerate()
            .map(|(index, event)| EventEntry {
                event,
                version: version(),
                expanded: false,
                has_detail: false,
                contract: None,
                selected: index == selected,
                link_highlight: None,
            })
            .collect();
        let view = EventListView {
            chrome: PanelChrome {
                focused: true,
                workspace: None,
                agent: "codex".into(),
                model: "gpt-5.6-sol".into(),
                model_reported: true,
                effort: None,
                effort_reported: true,
                status: "running".into(),
                status_tone: StatusTone::Running,
                elapsed: None,
                suffix: None,
                session: None,
            },
            entries,
            activity: ActivityCounts::default(),
            all_events: false,
            show_minor: false,
            uncommitted_changes: false,
            usage: None,
            can_configure_launch: false,
            selection_name: "codex".into(),
            requested_offset,
            moved_backward: false,
            protocol: Protocol::default(),
            links: LinkDisplay::Compact,
            search: SearchView {
                query: None,
                typing: false,
            },
            status: EventListStatus::Idle { reason: None },
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 10)).unwrap();
        let mut offset = 0;
        terminal
            .draw(|frame| {
                offset = render(frame, &view, frame.area()).effective_offset;
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect();
        (rows, offset)
    }

    fn numbered(count: usize) -> Vec<AgentEvent> {
        (0..count)
            .map(|n| AgentEvent::AgentMessage {
                text: format!("message {n}"),
            })
            .collect()
    }

    /// The list hands ratatui only the window it will draw, with the offset
    /// and the selection rebased onto it — so both have to be translated back
    /// out. The offset is persisted across frames, so getting this wrong would
    /// not flicker for a frame: it would move the session and keep it there.
    #[test]
    fn a_window_reports_its_offset_in_the_callers_numbering() {
        let events = numbered(500);

        let (rows, offset) = scrolled_screen(&events, 300, 300);

        assert_eq!(offset, 300, "the offset the caller gave back is its own");
        assert!(
            rows.iter().any(|row| row.contains("message 300")),
            "the selected entry is on screen: {rows:#?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("message 0")),
            "the top of the session is not: {rows:#?}"
        );
    }

    /// Windowing must not change which rows a screen shows. A list short
    /// enough that every entry fits is the case where the window is the whole
    /// list, and it has to read exactly as it did.
    #[test]
    fn a_list_shorter_than_the_viewport_still_shows_every_entry() {
        let events = numbered(4);

        let (rows, offset) = scrolled_screen(&events, 0, 0);

        assert_eq!(offset, 0);
        for n in 0..4 {
            assert!(
                rows.iter().any(|row| row.contains(&format!("message {n}"))),
                "message {n} is missing: {rows:#?}"
            );
        }
    }

    /// Scrolling is continuous: consecutive anchors show consecutive windows,
    /// with no entry skipped between two frames.
    #[test]
    fn consecutive_anchors_show_consecutive_windows() {
        let events = numbered(200);

        let (first, _) = scrolled_screen(&events, 100, 100);
        let (second, _) = scrolled_screen(&events, 101, 101);

        assert!(first.iter().any(|row| row.contains("message 101")));
        assert!(second.iter().any(|row| row.contains("message 101")));
    }

    /// One agent message, drawn with `search` typed into the `/` prompt.
    fn searched_screen(text: &str, search: SearchView<'_>) -> (Vec<String>, Vec<String>) {
        let event = AgentEvent::AgentMessage { text: text.into() };
        let view = EventListView {
            chrome: PanelChrome {
                focused: true,
                workspace: None,
                agent: "codex".into(),
                model: "gpt-5.6-sol".into(),
                model_reported: true,
                effort: None,
                effort_reported: true,
                status: "running".into(),
                status_tone: StatusTone::Running,
                elapsed: None,
                suffix: None,
                session: None,
            },
            entries: vec![EventEntry {
                event: &event,
                version: version(),
                expanded: false,
                has_detail: false,
                contract: None,
                selected: true,
                link_highlight: None,
            }],
            activity: ActivityCounts::default(),
            all_events: false,
            show_minor: false,
            uncommitted_changes: false,
            usage: None,
            can_configure_launch: false,
            selection_name: "codex".into(),
            requested_offset: 0,
            moved_backward: false,
            protocol: Protocol::default(),
            links: LinkDisplay::Compact,
            search,
            status: EventListStatus::Idle { reason: None },
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 8)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, &view, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect();
        // The marked text, row by row, so a test can say what was marked
        // without depending on where on the row it landed.
        let marked = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter(|x| buffer.cell((*x, y)).unwrap().bg == palette::ACCENT)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .collect();
        (rows, marked)
    }

    #[test]
    fn a_search_marks_the_matching_words_of_the_rows_on_screen() {
        let (_, marked) = searched_screen(
            "reworked the retry in src/retry.rs",
            SearchView {
                query: Some("retry"),
                typing: false,
            },
        );

        assert_eq!(marked.concat(), "retrysrc/retry.rs");
    }

    #[test]
    fn a_search_marks_nothing_until_three_characters_are_typed() {
        let (rows, marked) = searched_screen(
            "reworked the retry",
            SearchView {
                query: Some("re"),
                typing: true,
            },
        );

        assert_eq!(marked.concat(), "");
        // And the prompt says why nothing is marked yet.
        assert!(
            rows.concat().contains("/re▌ · 1 more character"),
            "{rows:?}"
        );
    }

    #[test]
    fn a_highlighted_link_does_not_repeat_the_selected_summary() {
        let event = AgentEvent::AgentMessage {
            text: "see [guide](https://example.com/guide)".into(),
        };
        let entry = EventEntry {
            event: &event,
            version: version(),
            expanded: true,
            has_detail: true,
            contract: None,
            selected: true,
            link_highlight: Some(0),
        };
        let render = EntryRender {
            protocol: Protocol::default(),
            links: LinkDisplay::Compact,
            search: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(80, 4)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    List::new(vec![entry_item(&entry, 80, 4, render)]),
                    frame.area(),
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let text = (0..buffer.area.height)
            .flat_map(|y| {
                (0..buffer.area.width).map(move |x| buffer.cell((x, y)).unwrap().symbol())
            })
            .collect::<String>();
        assert_eq!(text.matches("see guide").count(), 1);
        assert!(
            (0..buffer.area.width).any(|x| {
                let cell = buffer.cell((x, 0)).unwrap();
                cell.symbol() == "g" && cell.bg == palette::LINK_HIGHLIGHT_BACKGROUND
            }),
            "the selected link remains visible on the summary"
        );
    }

    #[test]
    fn token_counts_read_as_k_and_m_past_a_thousand() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(9_450), "9.4k");
        assert_eq!(format_tokens(126_400), "126k");
        assert_eq!(format_tokens(1_350_000), "1.3M");
        assert_eq!(format_tokens(12_000_000), "12M");
    }

    #[test]
    fn wide_markdown_tables_wrap_the_widest_column_inside_the_borders() {
        let markdown = "| State | Explanation |\n\
                        |---|---|\n\
                        | stable | This deliberately long explanation wraps inside its cell |";
        let lines = markdown_block_lines(markdown, Style::default(), "    ");
        let wrapped = wrap_log_lines(lines, 36, 0);
        let rendered: Vec<String> = wrapped
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect()
            })
            .collect();

        assert!(wrapped.iter().all(|line| line.width() == 36));
        assert_eq!(
            rendered.first().and_then(|line| line.chars().nth(4)),
            Some('┌')
        );
        assert_eq!(
            rendered.last().and_then(|line| line.chars().nth(4)),
            Some('└')
        );
        assert!(rendered.iter().all(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with('│') || trimmed.matches('│').count() == 3
        }));

        let body: Vec<&String> = rendered
            .iter()
            .skip_while(|line| !line.trim_start().starts_with('├'))
            .skip(1)
            .take_while(|line| !line.trim_start().starts_with('└'))
            .collect();
        assert!(body.len() > 1, "the long cell should add table rows");
        assert_eq!(
            body.iter().filter(|line| line.contains("stable")).count(),
            1
        );
        assert!(body.iter().skip(1).any(|line| line.contains("inside")));
    }

    /// A live row can change height without the selection moving: streamed
    /// thinking replaces the previous thinking body, and task/tool completion
    /// replaces its in-flight row.  The viewport must remain anchored when a
    /// replacement is shorter; revealing older entries makes the interaction
    /// log appear to jump backwards even though the operator did nothing.
    #[test]
    fn shrinking_live_tail_entry_does_not_reveal_older_entries() {
        let viewport_height = 6;
        let selected = Some(5);

        // Five old one-line rows precede a five-line live row. With the status
        // tail after it, following the live row legitimately advances the
        // viewport to item 5.
        let tall_live_row = [1, 1, 1, 1, 1, 5, 1];
        let anchored = list_offset_with_scrolloff(
            0,
            selected,
            &mut &tall_live_row[..],
            viewport_height,
            false,
        );
        assert_eq!(anchored, 5);

        // The same selected row is replaced by a one-line update. No
        // navigation occurred, so its item anchor should not change.
        let short_live_row = [1, 1, 1, 1, 1, 1, 1];
        let after_update = list_offset_with_scrolloff(
            anchored,
            selected,
            &mut &short_live_row[..],
            viewport_height,
            false,
        );
        assert_eq!(after_update, anchored);
    }

    #[test]
    fn backward_navigation_still_restores_scrolloff_above_the_selection() {
        let heights = [1, 1, 1, 1, 1, 1, 1];
        let offset = list_offset_with_scrolloff(5, Some(4), &mut &heights[..], 6, true);

        assert_eq!(offset, 2, "two rows of scrolloff above the selection");
    }

    /// Counts which items the offset math asked about, so laziness can be
    /// asserted rather than assumed.
    struct Counting {
        heights: Vec<usize>,
        asked: std::collections::BTreeSet<usize>,
    }

    impl Heights for Counting {
        fn len(&self) -> usize {
            self.heights.len()
        }

        fn height(&mut self, index: usize) -> usize {
            self.asked.insert(index);
            self.heights[index]
        }
    }

    /// The whole point of asking one height at a time: a thousand-row session
    /// must not have to be rendered to decide where to scroll it.
    ///
    /// The math reads a window around the viewport — forward from the offset
    /// until the viewport is full, and at most `margin` rows back from it —
    /// so what it touches is bounded by the viewport, not by the session.
    #[test]
    fn the_offset_math_only_asks_about_items_near_the_viewport() {
        let mut heights = Counting {
            heights: vec![1; 1000],
            asked: Default::default(),
        };
        let viewport_height = 20;

        let offset =
            list_offset_with_scrolloff(500, Some(504), &mut heights, viewport_height, true);

        assert!(
            heights
                .asked
                .iter()
                .all(|&index| (480..540).contains(&index)),
            "asked about items far from the viewport: {:?}",
            heights.asked,
        );
        assert!(
            heights.asked.len() < 100,
            "asked about {} of 1000 items",
            heights.asked.len(),
        );
        // Still the same answer the eager version gave for a flat list.
        let flat = vec![1usize; 1000];
        assert_eq!(
            offset,
            list_offset_with_scrolloff(500, Some(504), &mut &flat[..], viewport_height, true),
        );
    }

    /// A list with no selection is the one case that needs a count rather than
    /// any height at all, and it must not start rendering to get one.
    #[test]
    fn an_unselected_list_asks_about_no_items_at_all() {
        let mut heights = Counting {
            heights: vec![1; 1000],
            asked: Default::default(),
        };

        let offset = list_offset_with_scrolloff(400, None, &mut heights, 20, false);

        assert_eq!(offset, 400);
        assert!(heights.asked.is_empty(), "{:?}", heights.asked);
    }

    /// The text of the rows an entry renders to, for comparing one render
    /// against another.
    fn rows_of(item: &ListItem<'static>) -> Vec<String> {
        let mut out = Vec::new();
        let mut buffer = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 120, 40));
        ratatui::widgets::Widget::render(List::new(vec![item.clone()]), buffer.area, &mut buffer);
        for row in 0..item.height() {
            let mut text = String::new();
            for column in 0..120 {
                text.push_str(buffer[(column, row as u16)].symbol());
            }
            out.push(text.trim_end().to_owned());
        }
        out
    }

    fn entry_of<'a>(event: &'a AgentEvent, version: EntryVersion) -> EventEntry<'a> {
        EventEntry {
            event,
            version,
            expanded: true,
            has_detail: true,
            contract: None,
            selected: false,
            link_highlight: None,
        }
    }

    fn render_of() -> EntryRender<'static> {
        EntryRender {
            protocol: Protocol::default(),
            links: LinkDisplay::Compact,
            search: None,
        }
    }

    /// The whole point of keying on the version: a row that has been rewritten
    /// is a different row, and must not be answered from what the previous
    /// version rendered to.
    ///
    /// This is the failure the cache could actually cause — a tool that has
    /// finished still drawn as running — so it is worth asserting directly
    /// rather than trusting the key by inspection.
    #[test]
    fn a_rewritten_entry_is_not_answered_with_its_previous_rendering() {
        let version = version();
        let started = AgentEvent::ToolStarted {
            id: "t1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo build\"}".into(),
        };
        let before = rows_of(&entry_item(
            &entry_of(&started, version),
            120,
            40,
            render_of(),
        ));

        // The same row, one revision later — exactly what `ingest` does when
        // the tool finishes.
        let completed = AgentEvent::ToolCompleted {
            id: "t1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo build\"}".into(),
            status: "error".into(),
            output: "could not compile".into(),
        };
        let rewritten = EntryVersion {
            revision: version.revision + 1,
            ..version
        };
        let after = rows_of(&entry_item(
            &entry_of(&completed, rewritten),
            120,
            40,
            render_of(),
        ));

        assert_ne!(before, after, "the finished tool must render as finished");
        assert!(
            after.iter().any(|row| row.contains("could not compile")),
            "the rewritten row shows the new event: {after:?}"
        );
    }

    /// A hit has to be indistinguishable from a build, or the cache is a
    /// second renderer that can disagree with the first.
    #[test]
    fn a_second_render_of_an_unchanged_entry_matches_the_first() {
        let version = version();
        let event = AgentEvent::AgentMessage {
            text: "a paragraph\n\nand a second one with `code` and a [link](https://e.com)".into(),
        };
        let first = rows_of(&entry_item(
            &entry_of(&event, version),
            120,
            40,
            render_of(),
        ));
        let second = rows_of(&entry_item(
            &entry_of(&event, version),
            120,
            40,
            render_of(),
        ));

        assert_eq!(first, second);
    }

    /// Width is in the key because nothing below the cache re-wraps: the rows
    /// it holds are already wrapped. A narrower pane has to rebuild them.
    #[test]
    fn a_narrower_pane_does_not_reuse_rows_wrapped_for_a_wider_one() {
        let version = version();
        let event = AgentEvent::AgentMessage {
            text: "a long enough sentence that it must wrap when the pane is narrow".into(),
        };
        let wide = entry_item(&entry_of(&event, version), 120, 40, render_of());
        let narrow = entry_item(&entry_of(&event, version), 24, 40, render_of());

        assert!(
            narrow.height() > wide.height(),
            "wrapping at 24 columns takes more rows than at 120"
        );
    }
}
