//! Typed-answer presentation, independent of application controllers.

use crate::chrome::{panel_block, PanelChrome};
use crate::palette;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_protocol::{Answer, AnswerValue, FileLocation};

pub struct AnswerView<'a> {
    pub chrome: PanelChrome,
    pub answer: Option<&'a Answer>,
    pub error: Option<&'a str>,
    pub selected: usize,
}

pub fn render(frame: &mut Frame, view: &AnswerView<'_>, area: Rect) {
    let chrome = PanelChrome {
        suffix: Some(title(view.answer)),
        ..view.chrome.clone()
    };
    let Some(answer) = view.answer else {
        let message = view
            .error
            .map(|error| format!("  {error}"))
            .unwrap_or_else(|| "  no answer yet".into());
        frame.render_widget(Paragraph::new(message).block(panel_block(&chrome)), area);
        return;
    };
    let lines = match answer.value.as_ref() {
        Some(value) => value_lines(value, view.selected),
        None => unsatisfied_lines(answer),
    };
    let viewport = usize::from(area.height.saturating_sub(2));
    let scroll = view.selected.saturating_add(1).saturating_sub(viewport) as u16;
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(&chrome))
            .scroll((scroll, 0)),
        area,
    );
}

pub fn title(answer: Option<&Answer>) -> String {
    let Some(answer) = answer else {
        return "answer".into();
    };
    let contract = answer.contract.as_str();
    match answer.value.as_ref() {
        Some(AnswerValue::Lines(items)) => format!("answer · {contract} · {}", count(items.len())),
        Some(AnswerValue::Files(items)) => format!("answer · {contract} · {}", count(items.len())),
        Some(_) => format!("answer · {contract}"),
        None => format!("answer · {contract} · unsatisfied"),
    }
}

fn count(count: usize) -> String {
    if count == 1 {
        "1 item".into()
    } else {
        format!("{count} items")
    }
}

fn value_lines(value: &AnswerValue, selected: usize) -> Vec<Line<'static>> {
    match value {
        AnswerValue::Text(text) => text
            .lines()
            .map(|line| Line::from(line.to_owned()))
            .collect(),
        AnswerValue::Lines(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| selectable(index == selected, vec![Span::raw(item.clone())]))
            .collect(),
        AnswerValue::Files(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| selectable(index == selected, file_spans(item)))
            .collect(),
        AnswerValue::Json(json) => serde_json::to_string_pretty(json)
            .unwrap_or_else(|_| json.to_string())
            .lines()
            .map(|line| Line::from(line.to_owned()))
            .collect(),
    }
}

fn file_spans(file: &FileLocation) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        file.located(),
        Style::default().fg(palette::ACCENT),
    )];
    if !file.description.is_empty() {
        spans.push(Span::styled(
            format!("  {}", file.description),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    }
    spans
}

fn selectable(selected: bool, mut spans: Vec<Span<'static>>) -> Line<'static> {
    let style = if selected {
        Style::default().bg(palette::SELECTION_BACKGROUND)
    } else {
        Style::default()
    };
    let mut output = vec![Span::styled(
        if selected { "▍ " } else { "  " },
        Style::default().fg(if selected {
            palette::SELECTION_MARKER
        } else {
            palette::INACTIVE
        }),
    )];
    output.append(&mut spans);
    Line::from(output).style(style)
}

fn unsatisfied_lines(answer: &Answer) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            answer
                .error
                .clone()
                .unwrap_or_else(|| "the reply did not satisfy the contract".into()),
            Style::default()
                .fg(palette::WARNING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "the agent replied:",
            Style::default().fg(palette::ADDITIONAL_INFO),
        )),
        Line::default(),
    ];
    lines.extend(
        answer
            .source
            .lines()
            .map(|line| Line::from(line.to_owned())),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::StatusTone;
    use ratatui::{backend::TestBackend, Terminal};
    use std::path::PathBuf;
    use styra_protocol::Contract;

    fn chrome() -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: Some("work".into()),
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

    fn screen(answer: Option<&Answer>, error: Option<&str>, selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &AnswerView {
                        chrome: chrome(),
                        answer,
                        error,
                        selected,
                    },
                    frame.area(),
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn files_show_count_locations_notes_and_selection() {
        let answer = Answer {
            contract: Contract::Files,
            value: Some(AnswerValue::Files(vec![
                FileLocation {
                    path: PathBuf::from("src/auth.rs"),
                    line: Some(12),
                    column: None,
                    description: "checks token".into(),
                },
                FileLocation {
                    path: PathBuf::from("src/session.rs"),
                    line: None,
                    column: None,
                    description: String::new(),
                },
            ])),
            error: None,
            source: String::new(),
        };
        let output = screen(Some(&answer), None, 1);
        assert!(output.contains("answer · files · 2 items"), "{output}");
        assert!(output.contains("src/auth.rs:12  checks token"), "{output}");
        assert!(output.contains("▍ src/session.rs"), "{output}");
    }

    #[test]
    fn json_is_pretty_printed() {
        let answer = Answer {
            contract: Contract::Json,
            value: Some(AnswerValue::Json(serde_json::json!({"crate": "styra"}))),
            error: None,
            source: String::new(),
        };
        assert!(screen(Some(&answer), None, 0).contains("\"crate\": \"styra\""));
    }

    #[test]
    fn unsatisfied_answer_keeps_error_and_source() {
        let answer = Answer {
            contract: Contract::Json,
            value: None,
            error: Some("invalid JSON".into()),
            source: "plain reply".into(),
        };
        let output = screen(Some(&answer), None, 0);
        assert!(output.contains("unsatisfied"));
        assert!(output.contains("invalid JSON"));
        assert!(output.contains("plain reply"));
    }

    #[test]
    fn missing_and_failed_fetches_are_distinct() {
        assert!(screen(None, None, 0).contains("no answer yet"));
        assert!(screen(None, Some("fetch failed"), 0).contains("fetch failed"));
    }
}
