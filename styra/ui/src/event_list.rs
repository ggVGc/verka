//! The main event list: each entry a summary line that grows inline when
//! expanded, plus the empty-list start screen and the trailing status tail.

use crate::chrome::{panel_block, PanelChrome};
use crate::code::{code_block_lines, is_error_diagnostic};
use crate::footer::{message_text_color, tag_color};
use crate::markdown::{
    markdown_block_lines_with_links, parse_inline_spans, structural_indent, LinkDisplay,
};
use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::time::Duration;
use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};
use styra_protocol::Contract;

const MAX_DETAIL_LINES: usize = 40;
const DETAIL_INDENT: &str = "    ";
const RUNNING_INDICATOR: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub struct EventEntry<'a> {
    pub event: &'a AgentEvent,
    pub expanded: bool,
    pub has_detail: bool,
    pub contract: Option<&'a Contract>,
    pub selected: bool,
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
    },
    Ended,
}

pub struct EventListView<'a> {
    pub chrome: PanelChrome,
    pub entries: Vec<EventEntry<'a>>,
    pub conversation_only: bool,
    pub usage: Option<(u64, u64, u64)>,
    pub can_configure_launch: bool,
    pub selection_name: String,
    pub requested_offset: usize,
    pub protocol: Protocol,
    pub links: LinkDisplay,
    pub status: EventListStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventListFeedback {
    pub effective_offset: usize,
}

pub struct EntryLogView<'a> {
    pub entries: Vec<EventEntry<'a>>,
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
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(Span::styled(
            " entry log · follows selection · E: close ",
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
    let items = view
        .entries
        .iter()
        .map(|entry| entry_item(entry, width, viewport, view.protocol, view.links));
    let limit = view
        .entries
        .len()
        .saturating_sub(viewport)
        .min(usize::from(u16::MAX)) as u16;
    let effective = view.requested_scroll.min(limit);
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
    if view.conversation_only {
        block = conversation_only_title(block);
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
    let mut items: Vec<ListItem> = view
        .entries
        .iter()
        .map(|entry| entry_item(entry, width, viewport_height, view.protocol, view.links))
        .collect();
    items.push(ListItem::new(status_tail(&view.status)));
    // Include the status tail when deciding whether scrolling would reveal
    // useful content. Otherwise moving past a tall entry can look attractive
    // merely because the algorithm cannot see the row waiting below it.
    let item_heights: Vec<usize> = items.iter().map(ListItem::height).collect();
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
        &item_heights,
        viewport_height,
    );
    clip_boundary_entry(
        &mut items,
        &view.entries,
        offset,
        viewport_height,
        width,
        view.protocol,
        view.links,
    );
    let list = List::new(items).block(block);
    *state.offset_mut() = offset;
    state.select(position);
    frame.render_stateful_widget(list, area, &mut state);
    EventListFeedback {
        effective_offset: state.offset(),
    }
}

/// Ratatui's `List` only renders complete items. If the next expanded entry is
/// taller than the rows left at the bottom of the viewport, it would therefore
/// disappear entirely even though some of its text could be shown. Rebuild
/// that boundary entry with the actual remaining row budget. When it is the
/// final entry, retain a row for the status tail whenever there is room for
/// both its summary and the tail.
fn clip_boundary_entry(
    items: &mut [ListItem<'static>],
    entries: &[EventEntry<'_>],
    offset: usize,
    viewport_height: usize,
    width: usize,
    protocol: Protocol,
    links: LinkDisplay,
) {
    let mut remaining = viewport_height;
    for item_index in offset..items.len() {
        let height = items[item_index].height();
        if height <= remaining {
            remaining -= height;
            continue;
        }
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
        items[item_index] = entry_item_with_max_rows(entry, width, max_rows, protocol, links);
        return;
    }
}

/// Keep the selected item within a small margin of the viewport edges, like
/// vim's `scrolloff`, without throwing away visible content just to preserve
/// that margin. Heights are rendered rows rather than item counts so wrapped
/// summaries and expanded details do not break the calculation.
fn list_offset_with_scrolloff(
    current: usize,
    selected: Option<usize>,
    heights: &[usize],
    viewport_height: usize,
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
    let mut rows_through_selection = heights[offset..=selected].iter().sum::<usize>();
    while offset < selected && rows_through_selection > viewport_height {
        rows_through_selection = rows_through_selection.saturating_sub(heights[offset]);
        offset += 1;
    }

    // Moving upward may have put the selection against the top. Pull earlier
    // items back in while they fit and do not reduce the number of occupied
    // rows (they can displace content at the bottom of the viewport).
    while offset > 0 && rows_before_selection(offset, selected, heights) < margin {
        let candidate = offset - 1;
        if heights[candidate..=selected].iter().sum::<usize>() > viewport_height
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

fn visible_rows(offset: usize, heights: &[usize], viewport_height: usize) -> usize {
    heights
        .iter()
        .skip(offset)
        .scan(0usize, |used, height| {
            if used.saturating_add(*height) > viewport_height {
                return None;
            }
            *used += *height;
            Some(*height)
        })
        .sum()
}

fn rows_before_selection(offset: usize, selected: usize, heights: &[usize]) -> usize {
    heights[offset..selected].iter().sum()
}

fn rows_after_selection(
    offset: usize,
    selected: usize,
    heights: &[usize],
    viewport_height: usize,
) -> usize {
    let mut used = 0usize;
    let mut after = 0usize;
    for (index, height) in heights.iter().enumerate().skip(offset) {
        if used.saturating_add(*height) > viewport_height {
            break;
        }
        used += *height;
        if index > selected {
            after += *height;
        }
    }
    after
}

/// Gaps shorter than this are not named: while output is streaming the figure
/// would flicker between `0s` and `1s` and say nothing. Only a real pause is
/// worth reporting.
const QUIET_THRESHOLD: Duration = Duration::from_secs(3);

/// A reason as the fragment that precedes "waiting…", or nothing at all
/// where the state is its own explanation.
fn why(reason: &Option<String>) -> String {
    reason
        .as_ref()
        .map(|reason| format!("{reason} · "))
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
        } => (running_tail(*elapsed, *quiet, *events), palette::WARNING),
        // Idle carries no elapsed figure: nothing is happening, so a
        // climbing counter only draws the eye to a number that means nothing.
        EventListStatus::Idle { reason } => (
            format!("  ── idle · {}waiting for your message ──", why(reason)),
            palette::SUCCESS,
        ),
        EventListStatus::Background { elapsed } => (
            format!(
                "  ── idle {} · background work still running ──",
                format_duration(*elapsed)
            ),
            palette::WARNING,
        ),
        EventListStatus::Stopped { elapsed, reason } => (
            format!(
                "  ── paused {} · {}waiting for your next message ──",
                format_duration(*elapsed),
                why(reason)
            ),
            palette::INACTIVE,
        ),
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
        RUNNING_INDICATOR[events % RUNNING_INDICATOR.len()],
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
    protocol: Protocol,
    links: LinkDisplay,
) -> ListItem<'static> {
    entry_item_with_max_rows(
        entry,
        width,
        viewport_height.saturating_sub(1).max(1),
        protocol,
        links,
    )
}

fn entry_item_with_max_rows(
    entry: &EventEntry<'_>,
    width: usize,
    max_rows: usize,
    protocol: Protocol,
    links: LinkDisplay,
) -> ListItem<'static> {
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
        return ListItem::new(vec![with_selection_backdrop(row, entry.selected)]);
    }
    let mut lines = vec![summary];
    let mut detail = detail_lines_with_links(entry.event, protocol, None, links);
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
    let mut wrapped: Vec<Line<'static>> = lines
        .into_iter()
        .enumerate()
        .flat_map(|(index, line)| {
            let continuation_indent = if index == 0 {
                summary_indent
            } else {
                DETAIL_INDENT.len()
            };
            wrap_rendered(line, width, continuation_indent)
        })
        .collect();
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
    if let Some(first) = wrapped.first_mut() {
        *first = with_selection_backdrop(std::mem::take(first), entry.selected);
    }
    ListItem::new(wrapped)
}

/// Mark the selection by backing its first row — and only that row — with
/// [`palette::SELECTION_BACKGROUND`]. An expanded entry's detail body keeps the plain
/// background, so the highlight reads as one line rather than as a block.
/// The style sits on the [`Line`], not on its spans, so the fill runs to the
/// full width of the row instead of stopping at the end of the text.
fn with_selection_backdrop(line: Line<'static>, selected: bool) -> Line<'static> {
    if !selected {
        return line;
    }
    let style = line.style.bg(palette::SELECTION_BACKGROUND);
    line.style(style)
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
        spans.extend(parse_inline_spans(display_summary, summary_style));
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
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text_color = message_text_color(event.tag());
    let suspicious_shell = suspicious_shell_success(event);
    for block in protocol.presented_detail(event, PresentationMode::Pretty) {
        match block {
            DetailBlock::Text(text) => {
                let base_style = Style::default().fg(text_color);
                lines.extend(markdown_block_lines_with_links(
                    &text,
                    base_style,
                    DETAIL_INDENT,
                    links,
                ));
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

fn conversation_only_title(
    block: ratatui::widgets::Block<'static>,
) -> ratatui::widgets::Block<'static> {
    block.title_bottom(Line::from(Span::styled(
        " conversation only ",
        Style::default()
            .fg(palette::ACCENT)
            .add_modifier(Modifier::BOLD),
    )))
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
    use super::{format_tokens, list_offset_with_scrolloff};

    #[test]
    fn token_counts_read_as_k_and_m_past_a_thousand() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1.0k");
        assert_eq!(format_tokens(9_450), "9.4k");
        assert_eq!(format_tokens(126_400), "126k");
        assert_eq!(format_tokens(1_350_000), "1.3M");
        assert_eq!(format_tokens(12_000_000), "12M");
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
            &tall_live_row,
            viewport_height,
        );
        assert_eq!(anchored, 5);

        // The same selected row is replaced by a one-line update. No
        // navigation occurred, so its item anchor should not change.
        let short_live_row = [1, 1, 1, 1, 1, 1, 1];
        let after_update = list_offset_with_scrolloff(
            anchored,
            selected,
            &short_live_row,
            viewport_height,
        );
        assert_eq!(after_update, anchored);
    }
}
