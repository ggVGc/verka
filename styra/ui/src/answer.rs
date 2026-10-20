//! Typed-answer presentation, independent of application controllers.

use crate::chrome::{panel_block, PanelChrome};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_protocol::{Answer, AnswerValue, FileLocation};

/// Internal links name a section in the original agent message.
pub const LINK_PREFIX: &str = "styra:answer:";

pub fn link_section(destination: &str) -> Option<usize> {
    destination.strip_prefix(LINK_PREFIX)?.parse().ok()
}

/// Complete sections only: unfinished streamed content stays readable.
pub fn sections(message: &str) -> impl Iterator<Item = (usize, usize)> + '_ {
    use styra_protocol::contract::{CLOSE, OPEN};
    let mut offset = 0;
    std::iter::from_fn(move || {
        let start = offset + message[offset..].find(OPEN)?;
        let body = start + OPEN.len();
        let end = body + message[body..].find(CLOSE)? + CLOSE.len();
        offset = end;
        Some((start, end))
    })
}

/// Use the same replacement for rendering and link navigation. The original
/// event remains available for parsing, copying and the raw view.
pub fn presented_detail(
    event: &styra_protocol::event::AgentEvent,
    protocol: styra_protocol::event::Protocol,
) -> Vec<styra_protocol::event::DetailBlock> {
    use styra_protocol::event::{AgentEvent, DetailBlock, PresentationMode};
    if let AgentEvent::AgentMessage { text } = event {
        let mut output = String::new();
        let mut offset = 0;
        for (index, (start, end)) in sections(text).enumerate() {
            output.push_str(&text[offset..start]);
            output.push_str(&format!("[answer]({LINK_PREFIX}{index})"));
            offset = end;
        }
        if offset > 0 {
            output.push_str(&text[offset..]);
            return vec![DetailBlock::Text(output)];
        }
    }
    protocol.presented_detail(event, PresentationMode::Pretty)
}

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
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::default().fg(if view.error.is_some() {
                    theme::ERROR
                } else {
                    theme::TEXT
                }))
                .block(panel_block(&chrome)),
            area,
        );
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

/// The answer's full content without a selectable row, for link previews.
pub(crate) fn preview_lines(answer: &Answer) -> Vec<Line<'static>> {
    match answer.value.as_ref() {
        Some(value) => value_lines(value, usize::MAX),
        None => unsatisfied_lines(answer),
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
        Style::default().fg(theme::ACCENT),
    )];
    if !file.description.is_empty() {
        spans.push(Span::styled(
            format!("  {}", file.description),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    spans
}

fn selectable(selected: bool, mut spans: Vec<Span<'static>>) -> Line<'static> {
    let style = if selected {
        Style::default().bg(theme::SELECTION_BACKGROUND)
    } else {
        Style::default()
    };
    let mut output = vec![Span::styled(
        if selected { "▍ " } else { "  " },
        Style::default().fg(if selected {
            theme::SELECTION_MARKER
        } else {
            theme::INACTIVE
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
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "the agent replied:",
            Style::default().fg(theme::ADDITIONAL_INFO),
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
            worktree: None,
            source_branch: None,
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
    fn answer_sections_render_as_links_and_leave_surrounding_prose() {
        use styra_protocol::event::{AgentEvent, DetailBlock, Protocol};
        let text = "Before\n<styra:answer>\nsecret.rs\n</styra:answer>\nAfter";
        let event = AgentEvent::AgentMessage { text: text.into() };
        let blocks = presented_detail(&event, Protocol::CodexJsonl);
        let DetailBlock::Text(output) = &blocks[0] else {
            panic!("expected text")
        };
        assert_eq!(output, "Before\n[answer](styra:answer:0)\nAfter");
        assert_eq!(crate::markdown::markdown_link_count(output), 1);
        assert_eq!(
            crate::markdown::markdown_link_destination(output, 0).as_deref(),
            Some("styra:answer:0")
        );
        let lines = crate::event_list::message_rows(
            &event,
            None,
            Default::default(),
            80,
            Protocol::CodexJsonl,
            crate::markdown::LinkDisplay::Compact,
        );
        let screen = lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!screen.contains("secret.rs"), "{screen}");
        assert!(screen.contains("answer"), "{screen}");
        assert!(
            screen.contains("Before") && screen.contains("After"),
            "{screen}"
        );
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
                    end_line: None,
                    description: "checks token".into(),
                },
                FileLocation {
                    path: PathBuf::from("src/session.rs"),
                    line: None,
                    column: None,
                    end_line: None,
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
