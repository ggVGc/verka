//! Styra-wire and provider-native raw record presentation.

use crate::chrome::{panel_block, PanelChrome};
use crate::palette;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};
use styra_protocol::{Direction as WireDirection, RawLine};

pub enum RawSource<'a> {
    Styra(&'a [RawLine]),
    Provider(&'a [String]),
}

pub struct RawView<'a> {
    pub chrome: PanelChrome,
    pub source: RawSource<'a>,
    pub selected: usize,
    pub requested_preview_scroll: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RawFeedback {
    pub preview_limit: u16,
    pub effective_preview_scroll: u16,
}

pub fn render(frame: &mut Frame, view: &RawView<'_>, area: Rect) -> RawFeedback {
    let (len, provider) = match &view.source {
        RawSource::Styra(lines) => (lines.len(), false),
        RawSource::Provider(lines) => (lines.len(), true),
    };
    let selected = view.selected.min(len.saturating_sub(1));
    let (list_area, preview_area) = if len == 0 {
        (area, None)
    } else {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);
        (panes[0], Some(panes[1]))
    };
    let mut chrome = view.chrome.clone();
    chrome.suffix = Some(if provider {
        "raw · provider".into()
    } else {
        "raw".into()
    });
    let bottom = if provider {
        " provider native · v: Styra wire "
    } else {
        " Styra wire · v: provider native "
    };
    let block = panel_block(&chrome).title_bottom(Line::from(Span::styled(
        bottom,
        Style::default().fg(palette::MUTED_TEXT),
    )));
    if len == 0 {
        let empty = if provider {
            "  provider session contains no records"
        } else {
            "  no wire traffic yet"
        };
        frame.render_widget(Paragraph::new(empty).block(block), list_area);
        return RawFeedback::default();
    }
    let items = match &view.source {
        RawSource::Styra(lines) => lines
            .iter()
            .enumerate()
            .map(|(index, line)| ListItem::new(wire_line(line, index == selected)))
            .collect::<Vec<_>>(),
        RawSource::Provider(lines) => lines
            .iter()
            .enumerate()
            .map(|(index, line)| ListItem::new(provider_line(line, index == selected)))
            .collect::<Vec<_>>(),
    };
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(palette::SELECTION_BACKGROUND));
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, list_area, &mut state);

    let selected_text = match &view.source {
        RawSource::Styra(lines) => lines.get(selected).map(|line| line.text.as_str()),
        RawSource::Provider(lines) => lines.get(selected).map(String::as_str),
    };
    let lines = preview_lines(selected_text);
    let preview_area = preview_area.expect("non-empty raw source has preview");
    let limit = scroll_limit(
        &lines,
        preview_area.width.saturating_sub(2),
        preview_area.height.saturating_sub(2),
    );
    let effective = view.requested_preview_scroll.min(limit);
    let title = if provider {
        " provider entry "
    } else {
        " entry "
    };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(palette::INACTIVE))
        .title(Span::styled(
            title,
            Style::default().fg(palette::MUTED_TEXT),
        ));
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((effective, 0)),
        preview_area,
    );
    RawFeedback {
        preview_limit: limit,
        effective_preview_scroll: effective,
    }
}

fn scroll_limit(lines: &[Line<'_>], width: u16, height: u16) -> u16 {
    Paragraph::new(lines.to_vec())
        .wrap(Wrap { trim: false })
        .line_count(width.max(1))
        .saturating_sub(usize::from(height))
        .min(usize::from(u16::MAX)) as u16
}

fn preview_lines(text: Option<&str>) -> Vec<Line<'static>> {
    let Some(text) = text else {
        return vec![Line::from(Span::styled(
            "  no entry selected",
            Style::default().fg(palette::MUTED_TEXT),
        ))];
    };
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => json_lines(&value),
        Err(_) => text
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    line.to_owned(),
                    Style::default().fg(palette::TEXT),
                ))
            })
            .collect(),
    }
}

fn provider_line(text: &str, selected: bool) -> Line<'static> {
    let color = if selected {
        palette::WARNING
    } else {
        palette::TEXT
    };
    let spans = tagged(text)
        .map(|(tag, detail)| {
            vec![
                Span::styled(
                    pad_tag(&tag),
                    Style::default().fg(if selected {
                        palette::WARNING
                    } else {
                        palette::ADDITIONAL_INFO
                    }),
                ),
                Span::styled(detail, Style::default().fg(color)),
            ]
        })
        .unwrap_or_else(|| vec![Span::styled(text.to_owned(), Style::default().fg(color))]);
    Line::from(spans)
}

fn wire_line(line: &RawLine, selected: bool) -> Line<'static> {
    let (marker, default_marker) = match line.direction {
        WireDirection::ToAgent => ("» ", palette::ACCENT),
        WireDirection::FromAgent => ("« ", palette::SUCCESS),
    };
    let color = if selected {
        palette::WARNING
    } else {
        palette::TEXT
    };
    let mut spans = vec![Span::styled(
        marker,
        Style::default().fg(if selected {
            palette::WARNING
        } else {
            default_marker
        }),
    )];
    if let Some((tag, detail)) = tagged(&line.text) {
        spans.push(Span::styled(
            pad_tag(&tag),
            Style::default().fg(if selected {
                palette::WARNING
            } else {
                palette::ADDITIONAL_INFO
            }),
        ));
        spans.push(Span::styled(detail, Style::default().fg(color)));
    } else {
        spans.push(Span::styled(line.text.clone(), Style::default().fg(color)));
    }
    Line::from(spans)
}

fn tagged(text: &str) -> Option<(String, String)> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    entry_tag(&value).map(|tag| (tag, entry_detail(&value)))
}

const TAG_WIDTH: usize = 18;
fn pad_tag(tag: &str) -> String {
    format!("{tag:<TAG_WIDTH$} ")
}
fn entry_tag(value: &serde_json::Value) -> Option<String> {
    if let Some(method) = value.get("method").and_then(serde_json::Value::as_str) {
        return Some(method.into());
    }
    if let Some(kind) = value.get("type").and_then(serde_json::Value::as_str) {
        return Some(
            match value.get("subtype").and_then(serde_json::Value::as_str) {
                Some(subtype) => format!("{kind}:{subtype}"),
                None => kind.into(),
            },
        );
    }
    if value.get("result").is_some() {
        Some("result".into())
    } else if value.get("error").is_some() {
        Some("error".into())
    } else {
        None
    }
}
fn entry_detail(value: &serde_json::Value) -> String {
    for key in ["params", "message", "result", "error"] {
        if let Some(payload) = value.get(key) {
            return compact(payload);
        }
    }
    let Some(object) = value.as_object() else {
        return compact(value);
    };
    let rest = object
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "type" | "subtype" | "method"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    if rest.is_empty() {
        String::new()
    } else {
        compact(&serde_json::Value::Object(rest))
    }
}
fn compact(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        value => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn json_lines(value: &serde_json::Value) -> Vec<Line<'static>> {
    let mut writer = JsonWriter::default();
    writer.value(value, 0);
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
    fn value(&mut self, value: &serde_json::Value, indent: usize) {
        match value {
            serde_json::Value::Null => self.push("null", palette::JSON_LITERAL),
            serde_json::Value::Bool(value) => self.push(value.to_string(), palette::JSON_LITERAL),
            serde_json::Value::Number(value) => self.push(value.to_string(), palette::JSON_NUMBER),
            serde_json::Value::String(value) => {
                self.push(format!("{value:?}"), palette::JSON_STRING)
            }
            serde_json::Value::Array(items) => self.seq(
                items.iter(),
                items.len(),
                indent,
                '[',
                ']',
                |writer, value, indent| writer.value(value, indent),
            ),
            serde_json::Value::Object(items) => self.seq(
                items.iter(),
                items.len(),
                indent,
                '{',
                '}',
                |writer, (key, value), indent| {
                    writer.push(format!("{key:?}"), palette::JSON_KEY);
                    writer.push(": ", palette::JSON_PUNCTUATION);
                    writer.value(value, indent);
                },
            ),
        }
    }
    fn seq<T>(
        &mut self,
        items: impl Iterator<Item = T>,
        len: usize,
        indent: usize,
        open: char,
        close: char,
        mut write: impl FnMut(&mut Self, T, usize),
    ) {
        if len == 0 {
            self.push(format!("{open}{close}"), palette::JSON_PUNCTUATION);
            return;
        }
        self.push(open.to_string(), palette::JSON_PUNCTUATION);
        self.newline();
        for (index, item) in items.enumerate() {
            self.push("  ".repeat(indent + 1), palette::JSON_PUNCTUATION);
            write(self, item, indent + 1);
            if index + 1 < len {
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
    use super::*;
    use crate::chrome::StatusTone;
    use ratatui::{backend::TestBackend, Terminal};

    fn chrome() -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: None,
            agent: "codex".into(),
            model: "gpt".into(),
            model_reported: true,
            effort: None,
            effort_reported: false,
            status: "idle".into(),
            status_tone: StatusTone::Idle,
            elapsed: None,
            suffix: None,
            session: None,
        }
    }
    fn screen(view: RawView<'_>) -> (String, RawFeedback) {
        let mut terminal = Terminal::new(TestBackend::new(90, 14)).unwrap();
        let mut feedback = RawFeedback::default();
        terminal
            .draw(|frame| feedback = render(frame, &view, frame.area()))
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .chunks(90)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        (output, feedback)
    }
    #[test]
    fn styra_wire_has_direction_tags_and_pretty_preview() {
        let lines = vec![
            RawLine {
                at_ms: 0,
                direction: WireDirection::ToAgent,
                text: r#"{"method":"item/start","params":{"n":1}}"#.into(),
            },
            RawLine {
                at_ms: 0,
                direction: WireDirection::FromAgent,
                text: r#"{"type":"system","subtype":"init","ok":true}"#.into(),
            },
        ];
        let (output, _) = screen(RawView {
            chrome: chrome(),
            source: RawSource::Styra(&lines),
            selected: 1,
            requested_preview_scroll: 0,
        });
        for expected in ["»", "«", "item/start", "system:init", "\"ok\"", "true"] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
    }
    #[test]
    fn provider_records_have_no_wire_marker() {
        let lines = vec![r#"{"type":"session_meta","payload":{"id":"t-1"}}"#.to_owned()];
        let (output, _) = screen(RawView {
            chrome: chrome(),
            source: RawSource::Provider(&lines),
            selected: 0,
            requested_preview_scroll: 0,
        });
        assert!(output.contains("provider native"));
        assert!(output.contains("session_meta"));
        assert!(!output.contains("« session_meta"));
    }
    #[test]
    fn untagged_text_is_not_hidden() {
        let line = RawLine {
            at_ms: 0,
            direction: WireDirection::FromAgent,
            text: "not json".into(),
        };
        assert_eq!(wire_line(&line, false).spans[1].content, "not json");
    }
    #[test]
    fn preview_feedback_clamps_long_documents() {
        let value = serde_json::json!({"items": (0..40).collect::<Vec<_>>()});
        let lines = vec![RawLine {
            at_ms: 0,
            direction: WireDirection::FromAgent,
            text: value.to_string(),
        }];
        let (_, feedback) = screen(RawView {
            chrome: chrome(),
            source: RawSource::Styra(&lines),
            selected: 0,
            requested_preview_scroll: u16::MAX,
        });
        assert!(feedback.preview_limit > 0);
        assert_eq!(feedback.effective_preview_scroll, feedback.preview_limit);
    }
}
