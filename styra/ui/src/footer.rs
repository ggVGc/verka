//! Shared application footer layout and styling.

use crate::palette;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Muted,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub text: String,
    pub tone: Tone,
    pub bold: bool,
}

pub struct FooterView<'a> {
    pub working_directory: &'a str,
    pub quota: &'a [Segment],
    pub auto_retry: bool,
}

pub fn render(frame: &mut Frame, view: &FooterView<'_>, area: Rect) {
    let quota_width = view
        .quota
        .iter()
        .map(|segment| segment.text.width())
        .sum::<usize>()
        .min(area.width as usize) as u16;
    let retry = view.auto_retry.then_some(" R rate-limit retry: on ");
    let retry_width = retry
        .map(UnicodeWidthStr::width)
        .unwrap_or_default()
        .min(area.width.saturating_sub(quota_width) as usize) as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(quota_width),
            Constraint::Length(retry_width),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            view.working_directory.to_owned(),
            Style::default().fg(palette::ADDITIONAL_INFO),
        ))),
        chunks[0],
    );
    if !view.quota.is_empty() {
        let spans = view
            .quota
            .iter()
            .map(|segment| {
                let color = match segment.tone {
                    Tone::Muted => palette::MUTED_TEXT,
                    Tone::Warning => palette::WARNING,
                    Tone::Error => palette::ERROR,
                };
                let style = if segment.bold {
                    Style::default().fg(color).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(color)
                };
                Span::styled(segment.text.clone(), style)
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Line::from(spans)).right_aligned(), chunks[1]);
    }
    if let Some(retry) = retry {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                retry,
                Style::default().fg(palette::SUCCESS),
            )))
            .right_aligned(),
            chunks[2],
        );
    }
}

pub fn tag_color(tag: &str) -> Color {
    match tag {
        "agent" => palette::AGENT_TAG,
        "user" => palette::USER_TAG,
        "shell" => palette::SHELL_TAG,
        "tool" => palette::SPECIAL,
        "plan" | "files" => palette::INFO,
        "branch" => palette::ACCENT,
        "error" | "malformed" => palette::ERROR,
        _ => palette::ADDITIONAL_INFO,
    }
}

pub fn message_text_color(tag: &str) -> Color {
    match tag {
        "agent" => palette::AGENT_TEXT,
        "user" => palette::USER_TEXT,
        _ => palette::TEXT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn screen(width: u16, view: &FooterView<'_>) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
        terminal
            .draw(|frame| render(frame, view, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }
    #[test]
    fn shows_context_and_standing_state() {
        let quota = vec![Segment {
            text: " codex: 80%".into(),
            tone: Tone::Warning,
            bold: true,
        }];
        let output = screen(
            120,
            &FooterView {
                working_directory: "/workspace",
                quota: &quota,
                auto_retry: true,
            },
        );
        for expected in ["codex: 80%", "rate-limit retry: on", "/workspace"] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
    }
}
