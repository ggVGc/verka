//! The main event list: each entry a summary line that grows inline when
//! expanded, plus the empty-list start screen and the trailing status tail.

use super::markdown::markdown_block_lines;
use super::{
    conversation_only_title, format_duration, message_text_color, render_placeholder,
    render_preview, tag_color, view_block, DETAIL_INDENT, MAX_DETAIL_LINES, SELECTION_BG,
    SELECTION_MARKER,
};
use crate::app::{App, Progress, Status, View};
use crate::timeline::Entry;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;
use std::time::Duration;
use styra_server::event::{AgentEvent, DetailBlock, PresentationMode, Protocol};

/// Keep conversational prose readable on wide terminals. This includes the
/// human/agent marker and hanging indent; narrower panes still use all of the
/// space available to them.
const MAX_CONVERSATION_WIDTH: usize = 120;

pub(crate) fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let area = if app.show_preview && app.view == View::Events {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_preview(frame, app, chunks[1]);
        chunks[0]
    } else {
        area
    };

    let usage = app
        .latest_usage
        .as_ref()
        .map(|u| {
            format!(
                " in {} · out {} · cached {} ",
                u.input_tokens, u.output_tokens, u.cached_input_tokens
            )
        })
        .unwrap_or_default();
    let mut block = view_block(app, None).title_bottom(Line::from(usage).right_aligned());
    if app.timeline.conversation_only {
        block = conversation_only_title(block);
    }

    if app.timeline.entries.is_empty() {
        // Before anything is launched, the empty list is the start screen: the
        // one moment the agent, model, and effort are still open, so it says
        // what they are and how to change them instead of only waiting.
        let lines = if app.can_configure_launch() {
            vec![
                Line::from(vec![
                    Span::styled("  launching with ", Style::default().fg(Color::Gray)),
                    Span::styled(
                        app.selection.name(),
                        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(Span::styled(
                    "  press L to choose the default agent, model, and effort — or i to write the first message",
                    Style::default().fg(Color::Gray),
                )),
            ]
        } else {
            vec![Line::from(Span::styled(
                "  waiting for the agent — press i to send a message",
                Style::default().fg(Color::Gray),
            ))]
        };
        frame.render_widget(Paragraph::new(lines).block(block), area);
        return;
    }

    let visible: Vec<(usize, &Entry)> = app
        .timeline
        .entries
        .iter()
        .enumerate()
        .filter(|(idx, _)| app.timeline.is_visible(*idx))
        .collect();

    if visible.is_empty() {
        render_placeholder(
            frame,
            block,
            area,
            "  all entries hidden — press c or m to change filters",
        );
        return;
    }

    let width = area.width.saturating_sub(2) as usize;
    let viewport_height = area.height.saturating_sub(2) as usize;
    let mut items: Vec<ListItem> = visible
        .iter()
        .map(|(idx, entry)| {
            entry_item(
                entry,
                app.timeline.entry_expanded(*idx),
                width,
                viewport_height,
                app.selection.provider.protocol(),
                *idx == app.timeline.selected,
            )
        })
        .collect();
    items.push(ListItem::new(status_tail(app)));
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
    let position = visible
        .iter()
        .position(|(idx, _)| *idx == app.timeline.selected)
        .or_else(|| {
            visible
                .iter()
                .rposition(|(idx, _)| *idx < app.timeline.selected)
        });
    let offset = list_offset_with_scrolloff(
        app.timeline.list_offset.get(),
        position,
        &item_heights,
        viewport_height,
    );
    clip_boundary_entry(&mut items, &visible, offset, viewport_height, width, app);
    let list = List::new(items).block(block);
    *state.offset_mut() = offset;
    state.select(position);
    frame.render_stateful_widget(list, area, &mut state);
    app.timeline.list_offset.set(state.offset());
}

/// Ratatui's `List` only renders complete items. If the next expanded entry is
/// taller than the rows left at the bottom of the viewport, it would therefore
/// disappear entirely even though some of its text could be shown. Rebuild
/// that boundary entry with the actual remaining row budget. When it is the
/// final entry, retain a row for the status tail whenever there is room for
/// both its summary and the tail.
fn clip_boundary_entry(
    items: &mut [ListItem<'static>],
    visible: &[(usize, &Entry)],
    offset: usize,
    viewport_height: usize,
    width: usize,
    app: &App,
) {
    let mut remaining = viewport_height;
    for item_index in offset..items.len() {
        let height = items[item_index].height();
        if height <= remaining {
            remaining -= height;
            continue;
        }
        if remaining == 0 || item_index >= visible.len() {
            return;
        }

        let (entry_index, entry) = visible[item_index];
        let is_last_entry = item_index + 1 == visible.len();
        let max_rows = if is_last_entry && remaining > 1 {
            remaining - 1
        } else {
            remaining
        };
        items[item_index] = entry_item_with_max_rows(
            entry,
            app.timeline.entry_expanded(entry_index),
            width,
            max_rows,
            app.selection.provider.protocol(),
            entry_index == app.timeline.selected,
        );
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

/// Braille spinner frames. It steps once per event received rather than on a
/// timer: a still spinner then means nothing has come back, which is what
/// distinguishes a session that is still working from one that has hung.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Gaps shorter than this are not named: while output is streaming the figure
/// would flicker between `0s` and `1s` and say nothing. Only a real pause is
/// worth reporting.
const QUIET_THRESHOLD: Duration = Duration::from_secs(3);

fn status_tail(app: &App) -> Line<'static> {
    let progress = app.progress();
    let elapsed = format_duration(progress.in_status);
    let (text, color) = match app.status {
        Status::Pending => (
            "  … waiting for your first message".to_string(),
            Color::DarkGray,
        ),
        Status::Running => (running_tail(&progress), Color::Yellow),
        // Idle carries no elapsed figure: nothing is happening, so a
        // climbing counter only draws the eye to a number that means nothing.
        Status::Idle => (
            "  ── idle · waiting for your message ──".to_string(),
            Color::Green,
        ),
        Status::Background => (
            format!("  ── idle {elapsed} · background work still running ──"),
            Color::Yellow,
        ),
        Status::Stopped => (
            format!("  ── paused {elapsed} · waiting for your next message ──"),
            Color::DarkGray,
        ),
        _ => return Line::default(),
    };
    Line::from(Span::styled(text, Style::default().fg(color)))
}

/// The tail of a running turn: a spinner, how long the turn has been going,
/// and — once the agent has been quiet long enough for that to be a question —
/// how long since anything last came back from it.
fn running_tail(progress: &Progress) -> String {
    let mut text = format!(
        "  {} working {}",
        SPINNER[progress.events % SPINNER.len()],
        format_duration(progress.in_status)
    );
    if let Some(gap) = progress.since_event.filter(|gap| *gap >= QUIET_THRESHOLD) {
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
fn entry_item(
    entry: &Entry,
    expanded: bool,
    width: usize,
    viewport_height: usize,
    protocol: Protocol,
    selected: bool,
) -> ListItem<'static> {
    entry_item_with_max_rows(
        entry,
        expanded,
        width,
        viewport_height.saturating_sub(1).max(1),
        protocol,
        selected,
    )
}

fn entry_item_with_max_rows(
    entry: &Entry,
    expanded: bool,
    width: usize,
    max_rows: usize,
    protocol: Protocol,
    selected: bool,
) -> ListItem<'static> {
    let is_conversation = matches!(
        entry.event,
        AgentEvent::UserMessage { .. } | AgentEvent::AgentMessage { .. }
    );
    let summary_indent = if is_conversation { 2 } else { 0 };
    let width = if is_conversation {
        width.min(MAX_CONVERSATION_WIDTH)
    } else {
        width
    };
    let summary = selected_summary_line(
        summary_line(entry, expanded, entry.has_detail(), true, protocol),
        is_conversation,
        selected,
    );
    if !expanded {
        // A collapsed entry is always exactly one row: wrapping it would make
        // one long message push the rest of the session off screen, and the
        // available width shrinks whenever the preview pane opens.
        let row = truncate_line(summary, width, entry.has_detail());
        return ListItem::new(vec![with_selection_backdrop(row, selected)]);
    }
    let mut lines = vec![summary];
    let mut detail = detail_lines(&entry.event, protocol, None);
    if !detail.is_empty() {
        detail.remove(0);
    }
    if suspicious_shell_success(&entry.event) {
        detail.insert(
            0,
            Line::from(Span::styled(
                format!("{DETAIL_INDENT}reported success; output contains an error diagnostic"),
                Style::default().fg(Color::Yellow),
            )),
        );
    }
    if detail.len() > MAX_DETAIL_LINES {
        let hidden = detail.len() - MAX_DETAIL_LINES;
        detail.truncate(MAX_DETAIL_LINES);
        detail.push(Line::from(Span::styled(
            format!("{DETAIL_INDENT}… {hidden} more lines"),
            Style::default().fg(Color::Gray),
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
            wrap_line(line, width, continuation_indent)
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
                    .push(Span::styled(" …", Style::default().fg(Color::Gray)));
            }
        } else {
            let hidden = wrapped.len() - (max_rows - 1);
            wrapped.truncate(max_rows - 1);
            wrapped.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}… {hidden} more rows — press p for the full entry"),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    if let Some(first) = wrapped.first_mut() {
        *first = with_selection_backdrop(std::mem::take(first), selected);
    }
    ListItem::new(wrapped)
}

/// Mark the selection by backing its first row — and only that row — with
/// [`SELECTION_BG`]. An expanded entry's detail body keeps the plain
/// background, so the highlight reads as one line rather than as a block.
/// The style sits on the [`Line`], not on its spans, so the fill runs to the
/// full width of the row instead of stopping at the end of the text.
fn with_selection_backdrop(line: Line<'static>, selected: bool) -> Line<'static> {
    if !selected {
        return line;
    }
    let style = line.style.bg(SELECTION_BG);
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
            glyph.style = glyph.style.fg(SELECTION_MARKER);
        }
    } else if let Some(lead) = line.spans.get_mut(0) {
        *lead = Span::styled("• ", Style::default().fg(SELECTION_MARKER));
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
    kept.push(Span::styled("…", Style::default().fg(Color::DarkGray)));
    kept.extend(marker);
    Line::from(kept)
}

/// Word-wrap one logical line to `width` columns, preserving each span's
/// style across the break. Continuation rows use a hanging indent so message
/// text stays aligned with the text following its `«`/`»` marker (and detail
/// rows retain their body indent) instead of jumping to the far-left edge.
/// `List` does not wrap on its own, so long lines would otherwise be clipped.
pub(crate) fn wrap_line(
    line: Line<'static>,
    width: usize,
    continuation_indent: usize,
) -> Vec<Line<'static>> {
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

            if token_width > width {
                // A single token longer than the line: hard-split it.
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
pub(crate) fn summary_line(
    entry: &Entry,
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
                Style::default().fg(Color::Red),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" && suspicious_shell_success(&entry.event) =>
        {
            (
                Style::default().fg(tag_color(tag)),
                "⚠ ",
                Style::default().fg(Color::Yellow),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. }
            if tag == "shell" =>
        {
            (
                Style::default().fg(tag_color(tag)),
                "✓ ",
                Style::default().fg(Color::Green),
            )
        }
        _ if tag == "shell" => (Style::default().fg(tag_color(tag)), "", Style::default()),
        AgentEvent::ToolCompleted { status, .. } | AgentEvent::CommandCompleted { status, .. }
            if status == "error" =>
        {
            (
                Style::default().fg(Color::Red),
                "✗ ",
                Style::default().fg(Color::Red),
            )
        }
        AgentEvent::ToolCompleted { .. } | AgentEvent::CommandCompleted { .. } => (
            Style::default().fg(Color::White),
            "✓ ",
            Style::default().fg(Color::Green),
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
        spans.extend(super::markdown::parse_inline_spans(
            display_summary,
            summary_style,
        ));
    }
    if has_detail {
        spans.push(Span::styled(
            format!(" {marker}"),
            Style::default().fg(Color::DarkGray),
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
fn suspicious_shell_success(event: &AgentEvent) -> bool {
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

fn is_error_diagnostic(line: &str) -> bool {
    let line = line.trim_start().to_ascii_lowercase();
    line.starts_with("error:")
        || line.starts_with("error[")
        || line.starts_with("fatal:")
        || line.contains(": no such file or directory")
        || line.contains(": permission denied")
        || line.contains(": read-only file system")
        || line.ends_with(": command not found")
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
pub(crate) fn detail_lines(
    event: &AgentEvent,
    protocol: Protocol,
    cap: Option<usize>,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let text_color = message_text_color(event.tag());
    let suspicious_shell = suspicious_shell_success(event);
    for block in protocol.presented_detail(event, PresentationMode::Pretty) {
        match block {
            DetailBlock::Text(text) => {
                let base_style = Style::default().fg(text_color);
                lines.extend(markdown_block_lines(&text, base_style, DETAIL_INDENT));
            }
            DetailBlock::Code { text, .. } => {
                for line in text.lines() {
                    let color = if suspicious_shell && is_error_diagnostic(line) {
                        Color::Red
                    } else {
                        Color::White
                    };
                    lines.push(Line::from(vec![Span::styled(
                        format!("{DETAIL_INDENT}{line}"),
                        Style::default().fg(color),
                    )]));
                }
            }
        }
    }
    if let Some(cap) = cap {
        if lines.len() > cap {
            let hidden = lines.len() - cap;
            lines.truncate(cap);
            lines.push(Line::from(Span::styled(
                format!("{DETAIL_INDENT}… {hidden} more lines"),
                Style::default().fg(Color::Gray),
            )));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    #[test]
    fn list_scrolloff_moves_before_selection_reaches_an_edge() {
        let heights = vec![1; 20];

        assert_eq!(list_offset_with_scrolloff(0, Some(5), &heights, 8), 0);
        assert_eq!(list_offset_with_scrolloff(0, Some(6), &heights, 8), 1);
        assert_eq!(list_offset_with_scrolloff(5, Some(5), &heights, 8), 3);
    }

    #[test]
    fn list_scrolloff_counts_expanded_rows() {
        let heights = vec![1, 1, 5, 1, 1];

        assert_eq!(list_offset_with_scrolloff(0, Some(3), &heights, 8), 1);
    }

    #[test]
    fn list_scrolloff_keeps_a_full_viewport_before_a_short_selection() {
        // The long entry and the selected row fit exactly. Sacrificing the
        // long entry for two rows of scrolloff would leave only the selected
        // row and the status tail on screen.
        let heights = vec![1, 16, 1, 1];

        assert_eq!(list_offset_with_scrolloff(0, Some(2), &heights, 17), 1);
    }
    use styra_server::event::TokenUsage;

    fn progress(in_status: Duration, since_event: Option<Duration>) -> Progress {
        Progress {
            in_status,
            since_event,
            events: 0,
        }
    }

    #[test]
    fn a_running_turn_reports_how_long_it_has_been_working() {
        let tail = running_tail(&progress(
            Duration::from_secs(74),
            Some(Duration::from_secs(1)),
        ));

        assert!(tail.contains("working 1m14s"), "{tail}");
        // A gap of a second is streaming output, not a pause worth naming.
        assert!(!tail.contains("last update"), "{tail}");
    }

    #[test]
    fn a_quiet_running_turn_reports_the_gap_since_the_last_update() {
        let tail = running_tail(&progress(
            Duration::from_secs(300),
            Some(Duration::from_secs(42)),
        ));

        assert!(tail.contains("working 5m00s"), "{tail}");
        assert!(tail.contains("last update 42s ago"), "{tail}");
    }

    #[test]
    fn the_spinner_advances_only_when_an_event_arrives() {
        let mut early = progress(Duration::from_millis(0), None);
        let mut late = progress(Duration::from_millis(5_000), None);
        early.events = 1;
        late.events = 1;

        // Time passing on its own leaves the frame where it was.
        assert_eq!(
            running_tail(&early).chars().nth(2).unwrap(),
            running_tail(&late).chars().nth(2).unwrap()
        );

        late.events = 2;
        assert_ne!(
            running_tail(&early).chars().nth(2).unwrap(),
            running_tail(&late).chars().nth(2).unwrap()
        );
    }

    #[test]
    fn durations_are_formatted_compactly() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m00s");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59m59s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h00m");
        assert_eq!(format_duration(Duration::from_secs(7_500)), "2h05m");
    }

    #[test]
    fn a_running_session_shows_a_progress_tail_and_an_elapsed_title() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::UserMessage {
            text: "do the thing".into(),
        });

        let screen = rendered(&app);
        assert!(screen.contains("working 0s"), "{screen}");
        assert!(screen.contains("running 0s"), "{screen}");
    }

    #[test]
    fn an_idle_session_says_it_waits_without_counting_the_wait() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "done".into(),
        });
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage::default(),
        });
        app.note_progress();

        let screen = rendered(&app);
        assert!(
            screen.contains("idle · waiting for your message"),
            "{screen}"
        );
        assert!(!screen.contains("idle 0s"), "{screen}");
    }

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn expanded_and_selected_content_uses_the_selection_backdrop_not_white() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello\nworld".into(),
        });
        // `push_event` leaves the newest entry both selected (via follow) and,
        // once expanded, the case that used to flip to a reversed-white fill.
        app.timeline.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let backgrounds: Vec<Color> = buffer
            .content()
            .iter()
            .map(|cell| cell.style().bg.unwrap_or(Color::Reset))
            .collect();

        assert!(!backgrounds.contains(&Color::White));
        assert!(backgrounds.contains(&SELECTION_BG));
    }

    #[test]
    fn an_expanded_selected_entrys_detail_body_is_never_bold() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello\nworld".into(),
        });
        app.timeline.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let detail_row = (0..buffer.area.height)
            .find(|&y| {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect();
                row.contains("world")
            })
            .expect("no row contains the detail line");
        let is_bold = (0..buffer.area.width).any(|x| {
            buffer
                .cell((x, detail_row))
                .unwrap()
                .style()
                .add_modifier
                .contains(Modifier::BOLD)
        });
        assert!(
            !is_bold,
            "an expanded, selected entry's detail body must not be forced bold"
        );
    }

    #[test]
    fn only_the_selected_entrys_first_row_gets_the_selection_backdrop() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "one\ntwo".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "three\nfour".into(),
        });
        // `push_event` leaves the second (last) entry selected via follow;
        // both get expanded, but only the selected entry's summary row — not
        // its detail body — should be highlighted.
        app.timeline.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let row_containing = |text: &str| -> u16 {
            (0..buffer.area.height)
                .find(|&y| {
                    let row: String = (0..buffer.area.width)
                        .map(|x| buffer.cell((x, y)).unwrap().symbol())
                        .collect();
                    row.contains(text)
                })
                .unwrap_or_else(|| panic!("no row contains {text:?}"))
        };
        let row_has_selection_backdrop = |y: u16| {
            (0..buffer.area.width)
                .any(|x| buffer.cell((x, y)).unwrap().style().bg == Some(SELECTION_BG))
        };

        assert!(row_has_selection_backdrop(row_containing("three")));
        assert!(!row_has_selection_backdrop(row_containing("four")));
        assert!(!row_has_selection_backdrop(row_containing("one")));
        assert!(!row_has_selection_backdrop(row_containing("two")));
    }

    #[test]
    fn a_collapsed_entry_with_more_to_show_has_a_fold_marker() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world\nmore detail".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(screen.contains('▸'));
        assert!(screen.contains('«'));
    }

    #[test]
    fn an_entry_with_nothing_beyond_its_summary_has_no_fold_marker() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        // A single-line agent message: its detail body is identical to the
        // summary already shown, so there is nothing left to expand into.
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains("hello world"));
        assert!(!screen.contains('▸'));
        assert!(!screen.contains('▾'));
    }

    #[test]
    fn file_entries_name_the_action_taken() {
        let changed = AgentEvent::FileChanged {
            id: "f1".into(),
            paths: vec!["src/lib.rs".into()],
            diff: Some("@@ -1 +1 @@\n-old\n+new".into()),
            checkpoint: None,
            checkpoint_error: None,
        };
        assert_eq!(
            file_action_summary(&changed).as_deref(),
            Some("modified src/lib.rs")
        );

        let added = AgentEvent::DiffUpdated {
            diff: "diff --git a/new.rs b/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/new.rs"
                .into(),
        };
        assert_eq!(file_action_summary(&added).as_deref(), Some("added new.rs"));

        let deleted = AgentEvent::FileChanged {
            id: "f2".into(),
            paths: vec!["old.rs".into()],
            diff: Some("--- a/old.rs\n+++ /dev/null".into()),
            checkpoint: None,
            checkpoint_error: None,
        };
        assert_eq!(
            file_action_summary(&deleted).as_deref(),
            Some("deleted old.rs")
        );
    }

    #[test]
    fn a_truncated_single_line_summary_has_a_fold_marker_and_expands_to_the_full_text() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "z".repeat(500),
        });
        let collapsed = rendered(&app);
        assert!(collapsed.contains('…'));
        assert!(collapsed.contains('▸'));
        let collapsed_zs = collapsed.chars().filter(|&c| c == 'z').count();

        app.timeline.toggle_expand();
        let expanded = rendered(&app);
        // Expanding drops the arrow: there is nothing left to unfold.
        assert!(!expanded.contains('▾'));
        assert!(!expanded.contains('▸'));
        let expanded_zs = expanded.chars().filter(|&c| c == 'z').count();
        assert!(expanded_zs > collapsed_zs);
        // The full message appears exactly once — not the truncated summary
        // fragment followed by the whole message again.
        assert_eq!(expanded_zs, 500);
    }

    #[test]
    fn conversation_only_mode_renders_every_entry_expanded() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "z".repeat(500),
        });
        let collapsed = rendered(&app);
        assert!(collapsed.contains('▸'));
        assert!(collapsed.chars().filter(|&c| c == 'z').count() < 500);

        app.toggle_conversation_only();
        let expanded = rendered(&app);
        assert!(!expanded.contains('▸'));
        assert_eq!(expanded.chars().filter(|&c| c == 'z').count(), 500);

        // The filter did not consume the entry's own folding state.
        app.toggle_conversation_only();
        assert!(rendered(&app).contains('▸'));
    }

    #[test]
    fn a_shell_tool_gets_a_checkmark_only_when_it_completes() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
        });
        let running = rendered(&app);
        assert!(!running.contains('✓'));
        assert!(running.contains("Shell"));
        assert!(running.contains("cargo test"));

        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "ok".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains('✓'));
        assert!(screen.contains("Shell"));
        assert!(screen.contains("cargo test"));
        assert!(!screen.contains("tool     "));
        assert_eq!(screen.matches("Shell").count(), 1);

        let line = summary_line(
            &app.timeline.entries[0],
            app.timeline.entries[0].expanded,
            app.timeline.entries[0].has_detail(),
            true,
            app.selection.provider.protocol(),
        );
        let tag = line
            .spans
            .iter()
            .find(|span| span.content.contains("Shell"))
            .unwrap();
        let command = line
            .spans
            .iter()
            .find(|span| span.content.contains("cargo test"))
            .unwrap();
        assert_eq!(tag.style.fg, Some(tag_color("shell")));
        assert_eq!(command.style.fg, Some(tag_color("shell")));
    }

    #[test]
    fn a_failed_shell_tool_gets_a_cross() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: "{\"command\":\"cargo test\"}".into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "error".into(),
            output: "boom".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains('✗'));
        assert!(!screen.contains('✓'));
    }

    #[test]
    fn a_successful_shell_tool_with_an_error_diagnostic_gets_an_amber_warning() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("claude").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"ls -la /missing 2>&1 | head"}"#.into(),
        });
        app.push_event(AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "toolu_1".into(),
            detail: String::new(),
            status: "completed".into(),
            output: "ls: cannot access '/missing': No such file or directory".into(),
        });

        let line = summary_line(
            &app.timeline.entries[0],
            app.timeline.entries[0].expanded,
            app.timeline.entries[0].has_detail(),
            true,
            app.selection.provider.protocol(),
        );
        let warning = line
            .spans
            .iter()
            .find(|span| span.content.contains('⚠'))
            .expect("warning marker");
        assert_eq!(warning.style.fg, Some(Color::Yellow));
        assert!(!rendered(&app).contains('✓'));

        app.timeline.expand_all();
        let screen = rendered(&app);
        assert!(screen.contains("reported success; output contains an error diagnostic"));
    }

    #[test]
    fn cargo_errors_masked_by_a_pipeline_get_an_amber_warning() {
        let event = AgentEvent::ToolCompleted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"cargo test 2>&1 | tail -20"}"#.into(),
            status: "completed".into(),
            output: "error: failed to open `/tmp/fastrand.crate`\n\nCaused by:\n  Read-only file system (os error 30)".into(),
        };

        assert!(suspicious_shell_success(&event));
        let detail = detail_lines(&event, Protocol::ClaudeJsonl, None);
        let diagnostic = detail
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.content.contains("error: failed to open"))
            .expect("diagnostic output line");
        assert_eq!(diagnostic.style.fg, Some(Color::Red));
    }

    #[test]
    fn harmless_mentions_of_errors_remain_successful() {
        let event = AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "test error_handling ... ok\nall error cases passed".into(),
        };

        assert!(!suspicious_shell_success(&event));
    }

    #[test]
    fn a_nonzero_exit_is_a_failure_even_if_the_provider_says_completed() {
        let event = AgentEvent::CommandCompleted {
            command: "false".into(),
            status: "completed".into(),
            exit_code: Some(1),
            output: "error: expected failure".into(),
        };

        assert!(failed_shell_result(&event));
        assert!(!suspicious_shell_success(&event));
    }

    #[test]
    fn an_expanded_command_shows_detail_lines() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.timeline.expand_all();
        let screen = rendered(&app);
        assert!(!screen.contains('▾'));
        assert!(screen.contains("24 passed"));
    }

    #[test]
    fn an_expanded_entry_uses_the_pretty_provider_presentation() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("claude").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ToolStarted {
            id: "toolu_1".into(),
            name: "Bash".into(),
            detail: r#"{"command":"cargo test --all","description":"run the suite"}"#.into(),
        });

        app.timeline.expand_all();
        let screen = rendered(&app);
        assert!(screen.contains("cargo test --all"));
        assert!(!screen.contains("description"));
    }

    #[test]
    fn expanding_keeps_the_summary_on_the_first_row_without_repeating_it() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::CommandCompleted {
            command: "cargo test".into(),
            status: "completed".into(),
            exit_code: Some(0),
            output: "24 passed".into(),
        });
        app.timeline.expand_all();
        let screen = rendered(&app);
        // The command stays beside the Shell header when expanded, while its
        // matching first detail line is omitted so it is not printed twice.
        assert_eq!(screen.matches("cargo test").count(), 1);
        let command_row = screen
            .lines()
            .find(|line| line.contains("cargo test"))
            .unwrap();
        assert!(command_row.contains("Shell"));
        assert!(screen.contains("24 passed"));
    }

    /// `List` draws nothing at all — not even the neighbouring entries — when
    /// the selected item is taller than the viewport, so an expanded long
    /// message used to blank the entire log. It must stay clipped instead.
    #[test]
    fn an_expanded_entry_taller_than_the_viewport_is_clipped_not_dropped() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "an earlier message".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: (0..60).map(|i| format!("line number {i}\n")).collect(),
        });
        app.timeline.expand_all();

        let protocol = app.selection.provider.protocol();
        assert!(
            entry_item(
                &app.timeline.entries[1],
                app.timeline.entries[1].expanded,
                78,
                18,
                protocol,
                false
            )
            .height()
                <= 18
        );

        let screen = rendered(&app);
        assert!(screen.contains("line number 0"), "{screen}");
        assert!(screen.contains("press p for the full entry"), "{screen}");
    }

    #[test]
    fn selecting_after_a_long_message_keeps_the_message_visible() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "an earlier message".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: (0..60).map(|i| format!("line number {i}\n")).collect(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "the selected message".into(),
        });
        app.timeline.expand_all();

        let screen = rendered(&app);
        assert!(screen.contains("line number 0"), "{screen}");
        assert!(screen.contains("the selected message"), "{screen}");
    }

    #[test]
    fn a_long_expanded_entry_after_the_selection_still_shows_its_summary() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "the selected message".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "a short message in between".into(),
        });
        app.push_event(AgentEvent::AgentMessage {
            text: (0..60).map(|i| format!("tail line number {i}\n")).collect(),
        });
        app.timeline.expand_all();
        app.select_prev_line();
        app.select_prev_line();

        let screen = rendered(&app);
        assert!(screen.contains("the selected message"), "{screen}");
        assert!(screen.contains("tail line number 0"), "{screen}");
        assert!(screen.contains("tail line number 1"), "{screen}");
        assert!(screen.contains("press p for the full entry"), "{screen}");
    }

    #[test]
    fn clipping_an_expanded_entry_to_one_row_preserves_its_summary() {
        let entry = Entry {
            event: AgentEvent::AgentMessage {
                text: "the visible summary\nhidden detail".into(),
            },
            expanded: true,
            raw_index: None,
        };
        let item = entry_item_with_max_rows(&entry, true, 78, 1, Protocol::CodexAppServer, false);

        assert_eq!(item.height(), 1);
        let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
        terminal
            .draw(|frame| frame.render_widget(List::new(vec![item]), frame.area()))
            .unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("the visible summary"), "{screen}");
    }

    #[test]
    fn usage_is_shown_once_recorded() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::TurnCompleted {
            usage: TokenUsage {
                input_tokens: 12,
                output_tokens: 3,
                ..Default::default()
            },
        });
        let screen = rendered(&app);
        assert!(screen.contains("in 12"));
    }

    #[test]
    fn minor_events_are_omitted_from_the_list_when_hidden() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::ThreadStarted {
            thread_id: "t-1".into(),
            model: None,
            effort: None,
        });
        app.push_event(AgentEvent::AgentMessage {
            text: "hello world".into(),
        });
        // Hidden by default; no toggle needed to get here.
        assert!(!app.timeline.show_minor);
        let screen = rendered(&app);
        assert!(!screen.contains("t-1"));
        assert!(screen.contains("hello world"));
    }

    #[test]
    fn long_summary_lines_are_clipped_while_collapsed_and_wrap_once_expanded() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "word ".repeat(40),
        });
        let protocol = app.selection.provider.protocol();
        assert_eq!(
            entry_item(
                &app.timeline.entries[0],
                app.timeline.entries[0].expanded,
                40,
                18,
                protocol,
                false
            )
            .height(),
            1,
            "a collapsed entry must stay on a single row"
        );

        let screen = rendered(&app);
        assert!(screen.contains('…'), "{screen:?}");

        app.timeline.expand_all();
        let screen = rendered(&app);
        assert!(
            screen.matches("word").count() > 20,
            "expected wrapped continuation lines, only found: {screen:?}"
        );
    }

    #[test]
    fn wrapped_agent_messages_keep_a_hanging_indent() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        app.push_event(AgentEvent::AgentMessage {
            text: "one two three four five six seven eight nine ten eleven twelve".into(),
        });
        app.timeline.expand_all();

        let mut terminal = Terminal::new(TestBackend::new(24, 10)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, &app))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let message_rows = (1..9)
            .map(|y| {
                (1..23)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect::<String>()
            })
            .filter(|row| row.contains("one") || row.contains("four") || row.contains("seven"))
            .collect::<Vec<_>>();

        assert!(message_rows.len() >= 2, "{message_rows:?}");
        assert!(message_rows[0].starts_with("« "), "{message_rows:?}");
        assert!(
            message_rows.iter().skip(1).all(|row| row.starts_with("  ")),
            "{message_rows:?}"
        );
    }

    #[test]
    fn conversation_messages_are_capped_at_120_columns() {
        let mut app = App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        );
        let long_word = "x".repeat(MAX_CONVERSATION_WIDTH + 10);
        app.push_event(AgentEvent::UserMessage {
            text: long_word.clone(),
        });
        app.push_event(AgentEvent::AgentMessage { text: long_word });
        app.timeline.expand_all();

        let protocol = app.selection.provider.protocol();
        for entry in &app.timeline.entries {
            assert_eq!(
                entry_item(entry, entry.expanded, 200, 18, protocol, false).height(),
                2,
                "both human and agent messages should wrap at the conversation cap"
            );
        }
    }

    /// Before anything is launched, the empty list must name the launch and
    /// say what would be launched and how to change it, since that is the only
    /// moment the choice is still open.
    #[test]
    fn the_start_screen_names_the_launch_and_how_to_change_it() {
        let selection = styra_server::agent::Selection::parse("claude:opus/max").unwrap();
        let app = App::pending(selection);
        let screen = rendered(&app);
        assert!(screen.contains("claude:opus/max"), "{screen}");
        assert!(screen.contains("press L to choose"), "{screen}");
        assert!(screen.contains("? keybinds"), "{screen}");

        // A launched session shows the plain waiting message instead: its agent
        // and model are settled, so there is nothing to offer choosing.
        let app = App::new(
            styra_server::agent::Selection::parse("claude:opus/max").unwrap(),
            "s-1",
        );
        let screen = rendered(&app);
        assert!(screen.contains("waiting for the agent"), "{screen}");
        assert!(!screen.contains("press L to choose"), "{screen}");
    }
}
