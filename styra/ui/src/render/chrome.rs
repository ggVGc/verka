use super::*;
use crate::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

impl StopTone {
    pub fn color(self) -> ratatui::style::Color {
        match self {
            StopTone::Paused => theme::STOP_PAUSED,
            StopTone::Completed => theme::STOP_COMPLETED,
            StopTone::Abandoned => theme::STOP_ABANDONED,
            StopTone::Sealed => theme::STOP_SEALED,
            StopTone::Integrated => theme::STOP_INTEGRATED,
            StopTone::IntegrationPending => theme::STOP_INTEGRATION_PENDING,
            StopTone::RateLimited => theme::STOP_RATE_LIMITED,
            StopTone::Failed => theme::STOP_FAILED,
            StopTone::Exited => theme::STOP_EXITED,
            StopTone::ServerRestarted => theme::STOP_SERVER_RESTARTED,
            StopTone::Unknown => theme::STOP_UNKNOWN,
        }
    }
}

/// The bottom-border marker for a panel showing every event rather than the
/// default conversation only, saying too whether minor lifecycle events are
/// among them — with all events on, that is the other half of what is on
/// screen.
pub fn all_events_title(show_minor: bool) -> Line<'static> {
    let minor = if show_minor {
        "minor shown"
    } else {
        "minor hidden"
    };
    Line::from(Span::styled(
        format!(" all events · {minor} "),
        Style::default()
            .fg(theme::ACCENT)
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
            .fg(theme::UNCOMMITTED)
            .add_modifier(Modifier::BOLD),
    )))
}

/// Mark the panel that owns the keyboard with the grid's thick selection border.
pub fn focused_panel_block(chrome: &PanelChrome) -> Block<'static> {
    let block = panel_block(chrome);
    if !chrome.focused {
        return block;
    }
    block.border_type(BorderType::Thick).border_style(
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )
}

pub fn panel_block(chrome: &PanelChrome) -> Block<'static> {
    let tone = match chrome.status_tone {
        StatusTone::Pending => theme::INFO,
        StatusTone::Running => theme::RUNNING_STATUS,
        StatusTone::Idle => theme::SUCCESS,
        StatusTone::Background => theme::MUTED_WARNING,
        StatusTone::Stopped(why) => why.color(),
        StatusTone::Ended => theme::INACTIVE,
        StatusTone::Error => theme::ERROR,
    };
    let text = Style::default().fg(theme::MUTED_TEXT);
    let value = |reported| {
        Style::default().fg(if reported {
            theme::TEXT
        } else {
            theme::ADDITIONAL_INFO
        })
    };
    let mut spans = vec![Span::raw(" ")];
    if let Some(workspace) = &chrome.workspace {
        spans.push(Span::styled(
            workspace.clone(),
            Style::default()
                .fg(theme::WORKSPACE_NAME)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", text));
    }
    if let Some(session) = &chrome.session {
        spans.push(Span::styled(
            session.clone(),
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" · ", text));
    }
    if let Some(worktree) = &chrome.worktree {
        spans.push(Span::styled(
            worktree.clone(),
            Style::default().fg(theme::ACCENT),
        ));
        spans.push(Span::styled(" · ", text));
    }
    if let Some(source) = &chrome.source_branch {
        spans.push(Span::styled(
            format!("from {source}"),
            Style::default().fg(theme::ACCENT),
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
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if chrome.focused {
            theme::ACCENT
        } else {
            theme::INACTIVE
        }))
        .title(Line::from(spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};

    fn chrome(workspace: Option<&str>) -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: workspace.map(str::to_owned),
            session: None,
            worktree: None,
            source_branch: None,
            agent: "codex".into(),
            model: "default model".into(),
            model_reported: false,
            effort: None,
            effort_reported: false,
            status: "idle".into(),
            status_tone: StatusTone::Idle,
            elapsed: None,
            suffix: None,
        }
    }

    #[test]
    fn workspace_name_uses_the_navigator_heading_color() {
        let area = Rect::new(0, 0, 80, 3);
        let mut buffer = Buffer::empty(area);
        panel_block(&chrome(Some("payments"))).render(area, &mut buffer);

        for x in 2..10 {
            assert_eq!(buffer[(x, 0)].fg, theme::WORKSPACE_NAME);
        }
    }

    /// The point of the tones is telling one ending from another, so no two
    /// reasons may share a color.
    #[test]
    fn every_stop_reason_has_its_own_color() {
        let tones = [
            StopTone::Paused,
            StopTone::Completed,
            StopTone::Abandoned,
            StopTone::Sealed,
            StopTone::Integrated,
            StopTone::IntegrationPending,
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
