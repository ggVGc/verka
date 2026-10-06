//! Shared framed-panel chrome used by main application views.

use crate::palette;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Pending,
    Running,
    Idle,
    Background,
    Stopped(StopTone),
    Error,
    Ended,
}

/// Why an interaction stopped, as far as its color goes. Shared by every view
/// that draws a stopped interaction, so one reason reads the same everywhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopTone {
    Paused,
    Completed,
    Abandoned,
    Sealed,
    RateLimited,
    Failed,
    Exited,
    ServerRestarted,
    Unknown,
}

impl StopTone {
    pub fn color(self) -> ratatui::style::Color {
        match self {
            StopTone::Paused => palette::STOP_PAUSED,
            StopTone::Completed => palette::STOP_COMPLETED,
            StopTone::Abandoned => palette::STOP_ABANDONED,
            StopTone::Sealed => palette::STOP_SEALED,
            StopTone::RateLimited => palette::STOP_RATE_LIMITED,
            StopTone::Failed => palette::STOP_FAILED,
            StopTone::Exited => palette::STOP_EXITED,
            StopTone::ServerRestarted => palette::STOP_SERVER_RESTARTED,
            StopTone::Unknown => palette::STOP_UNKNOWN,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PanelChrome {
    pub focused: bool,
    pub workspace: Option<String>,
    pub agent: String,
    pub model: String,
    pub model_reported: bool,
    pub effort: Option<String>,
    pub effort_reported: bool,
    pub status: String,
    pub status_tone: StatusTone,
    pub elapsed: Option<String>,
    pub suffix: Option<String>,
    pub session: Option<String>,
}

/// The bottom-border marker for a panel showing every event rather than the
/// default conversation only, saying too whether minor lifecycle events are
/// among them — with all events on, that is the other half of what is on
/// screen.
pub fn all_events_title(show_minor: bool) -> Line<'static> {
    let minor = if show_minor { "minor shown" } else { "minor hidden" };
    Line::from(Span::styled(
        format!(" all events · {minor} "),
        Style::default()
            .fg(palette::ACCENT)
            .add_modifier(Modifier::BOLD),
    ))
}

/// Mark a panel as showing an interaction that stopped working and left
/// uncommitted changes in its repository. It rides the bottom border of the
/// pane the operator is already reading, beside the other markers about what
/// is on screen, because it is what they have to decide about before sending
/// the agent off again — and because nothing else tells them: the agent's own
/// account of a turn routinely says it changed files without saying whether it
/// committed them.
pub fn uncommitted_title(block: Block<'static>) -> Block<'static> {
    block.title_bottom(Line::from(Span::styled(
        " uncommitted changes ",
        Style::default()
            .fg(palette::UNCOMMITTED)
            .add_modifier(Modifier::BOLD),
    )))
}

pub fn panel_block(chrome: &PanelChrome) -> Block<'static> {
    let tone = match chrome.status_tone {
        StatusTone::Pending => palette::INFO,
        StatusTone::Running => palette::RUNNING_STATUS,
        StatusTone::Idle => palette::SUCCESS,
        StatusTone::Background => palette::MUTED_WARNING,
        StatusTone::Stopped(why) => why.color(),
        StatusTone::Ended => palette::INACTIVE,
        StatusTone::Error => palette::ERROR,
    };
    let text = Style::default().fg(palette::MUTED_TEXT);
    let value = |reported| {
        Style::default().fg(if reported {
            palette::TEXT
        } else {
            palette::ADDITIONAL_INFO
        })
    };
    let mut spans = vec![Span::raw(" ")];
    if let Some(workspace) = &chrome.workspace {
        spans.push(Span::styled(
            workspace.clone(),
            Style::default()
                .fg(palette::TEXT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", text));
    }
    spans.push(Span::styled(format!("{} · ", chrome.agent), text));
    spans.push(Span::styled(
        chrome.model.clone(),
        value(chrome.model_reported),
    ));
    if let Some(effort) = &chrome.effort {
        spans.push(Span::styled(" · ", text));
        spans.push(Span::styled(effort.clone(), value(chrome.effort_reported)));
    }
    spans.push(Span::styled(" · ", text));
    spans.push(Span::styled("● ", Style::default().fg(tone)));
    spans.push(Span::styled(
        chrome.status.clone(),
        Style::default().fg(tone).add_modifier(Modifier::BOLD),
    ));
    if let Some(elapsed) = &chrome.elapsed {
        spans.push(Span::styled(
            format!(" {elapsed}"),
            Style::default().fg(tone),
        ));
    }
    spans.push(Span::styled(
        chrome
            .suffix
            .as_ref()
            .map(|suffix| format!(" · {suffix} "))
            .unwrap_or_else(|| " ".into()),
        text,
    ));
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if chrome.focused {
            palette::ACCENT
        } else {
            palette::INACTIVE
        }))
        .title(Line::from(spans));
    if let Some(session) = &chrome.session {
        block = block.title(
            Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    session.clone(),
                    Style::default()
                        .fg(palette::ACCENT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ])
            .right_aligned(),
        );
    }
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point of the tones is telling one ending from another, so no two
    /// reasons may share a color.
    #[test]
    fn every_stop_reason_has_its_own_color() {
        let tones = [
            StopTone::Paused,
            StopTone::Completed,
            StopTone::Abandoned,
            StopTone::Sealed,
            StopTone::RateLimited,
            StopTone::Failed,
            StopTone::Exited,
            StopTone::ServerRestarted,
            StopTone::Unknown,
        ];
        for (i, a) in tones.iter().enumerate() {
            for b in &tones[i + 1..] {
                assert_ne!(a.color(), b.color(), "{a:?} and {b:?}");
            }
        }
    }
}
