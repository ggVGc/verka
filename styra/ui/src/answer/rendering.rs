use super::*;
use crate::chrome::{panel_block, PanelChrome};
use crate::theme;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_protocol::{Answer, AnswerValue, FileLocation};

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
mod tests;
