//! The raw view: every wire line as one truncated row (so the list reads as
//! a dense timeline of the protocol instead of a wall of wrapped JSON), plus
//! a side panel that pretty-prints and syntax-highlights the selected line
//! in full — the two together give both the overview and the detail that a
//! single wrapped-line-per-row view couldn't.

use super::{palette, preview_scroll_limit, render_placeholder, view_block};
use crate::app::App;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use styra_server::Direction as WireDirection;

pub(crate) fn render_raw(frame: &mut Frame, app: &App, area: Rect) {
    let area = if app.raw.is_empty() {
        area
    } else {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        render_raw_preview(frame, app, chunks[1]);
        chunks[0]
    };

    let block = view_block(app, Some("raw"));

    if app.raw.is_empty() {
        render_placeholder(frame, block, area, "  no wire traffic yet");
        return;
    }

    // One `ListItem` per wire line and no wrapping: a line wider than the
    // area is simply clipped at the right edge by the buffer, which is the
    // truncation the list wants — the full text is always one key away in
    // the preview panel.
    let items: Vec<ListItem> = app
        .raw
        .iter()
        .enumerate()
        .map(|(idx, line)| ListItem::new(raw_line(line, idx == app.raw.selected_index())))
        .collect();
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(palette::SELECTION_BACKGROUND));
    let mut state = ListState::default();
    state.select(Some(app.raw.selected_index()));
    frame.render_stateful_widget(list, area, &mut state);
}

/// The raw view's side panel: the selected wire line, pretty-printed and
/// syntax-highlighted if it parses as JSON (as every line normally does),
/// or shown verbatim otherwise rather than hiding it behind a parse error.
fn render_raw_preview(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(Span::styled(
            " entry ",
            Style::default().fg(palette::MUTED_TEXT),
        ));
    let lines = raw_preview_lines(app);
    let scroll_limit = preview_scroll_limit(
        &lines,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    app.raw.preview.note_limit(scroll_limit);
    let scroll = app.raw.preview.clamped();
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

fn raw_preview_lines(app: &App) -> Vec<Line<'static>> {
    let Some(line) = app.raw.selected() else {
        return vec![Line::from(Span::styled(
            "  no wire traffic yet",
            Style::default().fg(palette::MUTED_TEXT),
        ))];
    };
    match serde_json::from_str::<serde_json::Value>(&line.text) {
        Ok(value) => json_lines(&value),
        Err(_) => line
            .text
            .lines()
            .map(|text| {
                Line::from(Span::styled(
                    text.to_owned(),
                    Style::default().fg(palette::TEXT),
                ))
            })
            .collect(),
    }
}

fn raw_line(line: &styra_server::RawLine, selected: bool) -> Line<'static> {
    let (marker, marker_color) = match line.direction {
        WireDirection::ToAgent => ("» ", palette::ACCENT),
        WireDirection::FromAgent => ("« ", palette::SUCCESS),
    };
    let text_color = if selected {
        palette::WARNING
    } else {
        palette::TEXT
    };
    let marker_color = if selected {
        palette::WARNING
    } else {
        marker_color
    };
    let mut spans = vec![Span::styled(marker, Style::default().fg(marker_color))];
    match serde_json::from_str::<serde_json::Value>(&line.text)
        .ok()
        .and_then(|value| entry_tag(&value).map(|tag| (tag, entry_detail(&value))))
    {
        Some((tag, detail)) => {
            let tag_color = if selected {
                palette::WARNING
            } else {
                palette::ADDITIONAL_INFO
            };
            spans.push(Span::styled(pad_tag(&tag), Style::default().fg(tag_color)));
            spans.push(Span::styled(detail, Style::default().fg(text_color)));
        }
        // A line that is not JSON, or whose shape neither decoder recognises,
        // has nothing better to show than itself.
        None => spans.push(Span::styled(
            line.text.clone(),
            Style::default().fg(text_color),
        )),
    }
    Line::from(spans)
}

/// The tag column's width. Wide enough for the codex method names actually on
/// the wire (`item/tool/call`, `turn/interrupt`) so their payloads still line
/// up in one column; a longer tag pushes its own payload right rather than
/// widening the column for every other row.
const TAG_WIDTH: usize = 18;

fn pad_tag(tag: &str) -> String {
    format!("{tag:<TAG_WIDTH$} ")
}

/// The name of the kind of entry a wire line is, as the agent's own protocol
/// names it: codex's JSON-RPC `method`, or Claude's `type` refined by the
/// `subtype` that carries the actual meaning of a `system` line. A JSON-RPC
/// reply carries neither, so it is named for the field that distinguishes a
/// result from a failure.
fn entry_tag(value: &serde_json::Value) -> Option<String> {
    if let Some(method) = value.get("method").and_then(serde_json::Value::as_str) {
        return Some(method.to_owned());
    }
    if let Some(wire_type) = value.get("type").and_then(serde_json::Value::as_str) {
        return Some(match value.get("subtype").and_then(serde_json::Value::as_str) {
            Some(subtype) => format!("{wire_type}:{subtype}"),
            None => wire_type.to_owned(),
        });
    }
    if value.get("result").is_some() {
        return Some("result".into());
    }
    if value.get("error").is_some() {
        return Some("error".into());
    }
    None
}

/// What is left of a tagged line once the envelope the tag already states is
/// removed: the payload the operator is actually reading the row for, on one
/// compact line. The entry panel still holds the whole line, so dropping the
/// envelope here loses nothing.
fn entry_detail(value: &serde_json::Value) -> String {
    for key in ["params", "message", "result", "error"] {
        if let Some(payload) = value.get(key) {
            return compact(payload);
        }
    }
    let Some(object) = value.as_object() else {
        return compact(value);
    };
    let rest: serde_json::Map<_, _> = object
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "type" | "subtype" | "method"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if rest.is_empty() {
        String::new()
    } else {
        compact(&serde_json::Value::Object(rest))
    }
}

fn compact(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Pretty-print and syntax-highlight a JSON value for the raw view's entry
/// panel: keys, strings, numbers, and `true`/`false`/`null` each get their
/// own color so a nested payload's shape reads at a glance instead of
/// requiring the operator to parse a single dense line by eye.
fn json_lines(value: &serde_json::Value) -> Vec<Line<'static>> {
    let mut writer = JsonWriter::default();
    writer.write_value(value, 0);
    writer.finish()
}

#[derive(Default)]
struct JsonWriter {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
}

impl JsonWriter {
    fn push(&mut self, text: impl Into<String>, color: Color) {
        self.current
            .push(Span::styled(text.into(), Style::default().fg(color)));
    }

    fn newline(&mut self) {
        self.lines
            .push(Line::from(std::mem::take(&mut self.current)));
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        if !self.current.is_empty() {
            self.newline();
        }
        self.lines
    }

    fn write_value(&mut self, value: &serde_json::Value, indent: usize) {
        match value {
            serde_json::Value::Null => self.push("null", palette::JSON_LITERAL),
            serde_json::Value::Bool(b) => self.push(b.to_string(), palette::JSON_LITERAL),
            serde_json::Value::Number(n) => self.push(n.to_string(), palette::JSON_NUMBER),
            serde_json::Value::String(s) => self.push(format!("{s:?}"), palette::JSON_STRING),
            serde_json::Value::Array(items) => self.write_seq(
                items.iter(),
                items.len(),
                indent,
                '[',
                ']',
                |w, item, indent| w.write_value(item, indent),
            ),
            serde_json::Value::Object(map) => self.write_seq(
                map.iter(),
                map.len(),
                indent,
                '{',
                '}',
                |w, (key, val), indent| {
                    w.push(format!("{key:?}"), palette::JSON_KEY);
                    w.push(": ", palette::JSON_PUNCTUATION);
                    w.write_value(val, indent);
                },
            ),
        }
    }

    /// Shared body for arrays and objects: an empty one collapses to `[]`/
    /// `{}` on the current line; otherwise each item gets its own indented
    /// line, comma-separated, between the open and close brackets on their
    /// own lines.
    fn write_seq<T>(
        &mut self,
        items: impl Iterator<Item = T>,
        len: usize,
        indent: usize,
        open: char,
        close: char,
        mut write_item: impl FnMut(&mut Self, T, usize),
    ) {
        if len == 0 {
            self.push(format!("{open}{close}"), palette::JSON_PUNCTUATION);
            return;
        }
        self.push(open.to_string(), palette::JSON_PUNCTUATION);
        self.newline();
        let item_indent = "  ".repeat(indent + 1);
        for (i, item) in items.enumerate() {
            self.push(item_indent.clone(), palette::JSON_PUNCTUATION);
            write_item(self, item, indent + 1);
            if i + 1 < len {
                self.push(",", palette::JSON_PUNCTUATION);
            }
            self.newline();
        }
        self.push(
            format!("{}{close}", "  ".repeat(indent)),
            palette::JSON_PUNCTUATION,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::super::testing::rendered;
    use super::*;

    #[test]
    fn raw_view_shows_wire_lines_with_direction_markers() {
        use styra_server::{Direction, RawLine};
        let mut app = testing::app("s1");
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::ToAgent,
            text: r#"{"op":"user_input"}"#.into(),
        });
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"type":"turn.started"}"#.into(),
        });
        app.toggle_raw();
        let screen = rendered(&app);
        // The main title (agent · model · effort · status · "raw") is long
        // and the list only gets 60% of the panel now that the entry preview
        // is always alongside it, so — as the Events view's own preview
        // panel test already does for its "preview" title — check for the
        // panel's own short title rather than one that can be clipped at a
        // narrow width.
        assert!(screen.contains("entry"));
        assert!(screen.contains('»'));
        assert!(screen.contains('«'));
        assert!(screen.contains("turn.started"));
    }

    #[test]
    fn long_raw_lines_are_truncated_in_the_list_but_shown_in_full_in_the_preview() {
        use styra_server::{Direction, RawLine};
        let mut app = testing::app("s1");
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: format!(
                r#"{{"type":"item.completed","text":"{}END"}}"#,
                "a".repeat(200)
            ),
        });
        app.toggle_raw();
        let screen = rendered(&app);
        assert!(
            screen.contains("item.completed"),
            "the start of the line is shown in the truncated list row"
        );
        assert!(
            screen.contains("END"),
            "the full text still reaches the entry preview panel"
        );
    }

    #[test]
    fn raw_rows_lead_with_the_entrys_protocol_name_and_drop_the_envelope() {
        use styra_server::{Direction, RawLine};
        let codex = RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"method":"item/completed","params":{"item":{"type":"agentMessage"}}}"#.into(),
        };
        let row = raw_line(&codex, false);
        assert_eq!(row.spans[1].content.trim(), "item/completed");
        assert_eq!(row.spans[2].content, r#"{"item":{"type":"agentMessage"}}"#);

        let claude = RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"type":"system","subtype":"init","session_id":"s-1"}"#.into(),
        };
        let row = raw_line(&claude, false);
        assert_eq!(row.spans[1].content.trim(), "system:init");
        assert_eq!(row.spans[2].content, r#"{"session_id":"s-1"}"#);
    }

    #[test]
    fn an_untagged_raw_line_still_shows_its_own_text() {
        use styra_server::{Direction, RawLine};
        let line = RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: "not json at all".into(),
        };
        let row = raw_line(&line, false);
        assert_eq!(row.spans[1].content, "not json at all");
    }

    #[test]
    fn raw_preview_pretty_prints_and_highlights_the_selected_line() {
        use styra_server::{Direction, RawLine};
        let mut app = testing::app("s1");
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"type":"turn.started","ok":true,"count":3}"#.into(),
        });
        app.toggle_raw();
        let screen = rendered(&app);
        assert!(screen.contains("\"type\""), "{screen}");
        assert!(screen.contains("turn.started"));
        assert!(screen.contains("true"));
        assert!(screen.contains('3'));
    }

    #[test]
    fn raw_view_navigates_and_previews_the_selected_line() {
        use styra_server::{Direction, RawLine};
        let mut app = testing::app("s1");
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"marker":"first"}"#.into(),
        });
        app.raw.push(RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: r#"{"marker":"second"}"#.into(),
        });
        app.toggle_raw();
        assert_eq!(app.raw.selected_index(), 1, "starts on the tail");
        assert!(rendered(&app).contains("second"));

        app.raw.select_prev();
        assert_eq!(app.raw.selected_index(), 0);
        assert!(rendered(&app).contains("first"));
    }

    #[test]
    fn the_selected_raw_lines_text_is_yellow() {
        use styra_server::{Direction, RawLine};
        let line = RawLine {
            at_ms: 0,
            direction: Direction::FromAgent,
            text: "hello".into(),
        };

        let selected = raw_line(&line, true);
        assert!(selected
            .spans
            .iter()
            .all(|span| span.style.fg == Some(palette::WARNING)));

        let unselected = raw_line(&line, false);
        assert!(!unselected
            .spans
            .iter()
            .any(|span| span.style.fg == Some(palette::WARNING)));
    }
}

