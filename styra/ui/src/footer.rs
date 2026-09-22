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
    pub help_key: &'a str,
    pub working_directory: &'a str,
    pub idle_interactions: usize,
    /// The interaction being shown has stopped working and left uncommitted
    /// changes in its repository. Reported at the bottom of the view the
    /// operator is already looking at, because it is what they have to decide
    /// about before sending the agent off again — and because nothing else
    /// tells them: the agent's own account of a turn routinely says it
    /// changed files without saying whether it committed them.
    pub uncommitted_changes: bool,
    pub quota: &'a [Segment],
    pub auto_retry: bool,
}

pub fn render(frame: &mut Frame, view: &FooterView<'_>, area: Rect) {
    let keybinds = format!(" {} keybinds", view.help_key);
    let keybinds_width = keybinds.width().min(area.width as usize) as u16;
    let idle = (view.idle_interactions > 0).then(|| {
        format!(
            " ^a {} interaction{} idle ",
            view.idle_interactions,
            if view.idle_interactions == 1 { "" } else { "s" }
        )
    });
    let idle_width = idle
        .as_deref()
        .map(UnicodeWidthStr::width)
        .unwrap_or_default()
        .min(area.width as usize) as u16;
    let uncommitted = view.uncommitted_changes.then_some(" uncommitted changes ");
    let uncommitted_width = uncommitted
        .map(UnicodeWidthStr::width)
        .unwrap_or_default()
        .min(area.width as usize) as u16;
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
    let directory_width = view.working_directory.width().min(
        area.width
            .saturating_sub(keybinds_width)
            .saturating_sub(idle_width)
            .saturating_sub(uncommitted_width)
            .saturating_sub(retry_width)
            .saturating_sub(quota_width) as usize,
    ) as u16;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(quota_width),
            Constraint::Length(retry_width),
            Constraint::Length(idle_width),
            Constraint::Length(uncommitted_width),
            Constraint::Length(directory_width),
        ])
        .split(area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            keybinds,
            Style::default().fg(palette::MUTED_TEXT),
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
    if let Some(idle) = idle {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                idle,
                Style::default().fg(palette::SUCCESS),
            )))
            .right_aligned(),
            chunks[3],
        );
    }
    if let Some(uncommitted) = uncommitted {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                uncommitted,
                Style::default()
                    .fg(palette::WARNING)
                    .add_modifier(Modifier::BOLD),
            )))
            .right_aligned(),
            chunks[4],
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            view.working_directory.to_owned(),
            Style::default().fg(palette::ADDITIONAL_INFO),
        )))
        .right_aligned(),
        chunks[5],
    );
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
                help_key: "?",
                working_directory: "/workspace",
                idle_interactions: 2,
                uncommitted_changes: false,
                quota: &quota,
                auto_retry: true,
            },
        );
        for expected in [
            "? keybinds",
            "codex: 80%",
            "rate-limit retry: on",
            "2 interactions idle",
            "/workspace",
        ] {
            assert!(output.contains(expected), "missing {expected}: {output}");
        }
    }
    /// What an idle agent left behind is only actionable if the operator is
    /// told about it where they are already looking.
    #[test]
    fn reports_work_the_idle_agent_left_uncommitted() {
        let view = FooterView {
            help_key: "?",
            working_directory: "/workspace",
            idle_interactions: 0,
            uncommitted_changes: true,
            quota: &[],
            auto_retry: false,
        };
        assert!(screen(120, &view).contains("uncommitted changes"));

        let clean = FooterView {
            uncommitted_changes: false,
            ..view
        };
        assert!(
            !screen(120, &clean).contains("uncommitted"),
            "a clean checkout takes no footer space"
        );
    }

    #[test]
    fn keybind_hint_survives_a_narrow_terminal() {
        let output = screen(
            30,
            &FooterView {
                help_key: "?",
                working_directory: "/a/very/long/directory",
                idle_interactions: 0,
                uncommitted_changes: false,
                quota: &[],
                auto_retry: false,
            },
        );
        assert!(output.contains("? keybinds"), "{output}");
    }
}
