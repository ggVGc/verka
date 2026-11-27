use super::*;
use crate::theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};
use ratatui::Frame;
use std::ops::Range;

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
            StopTone::ServerRestartedMidTurn => theme::STOP_SERVER_RESTARTED_MID_TURN,
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

/// Mark a panel as showing an interaction whose workspace is mounted
/// read-only — as a launch in its repository's main worktree is unless a
/// policy layer grants it. The agent can read the project but every edit it
/// tries fails, which reads as the agent misbehaving unless the pane it is
/// watched in says why.
pub fn read_only_title(block: Block<'static>) -> Block<'static> {
    block.title_bottom(Line::from(Span::styled(
        " read-only workspace ",
        Style::default()
            .fg(theme::WARNING)
            .add_modifier(Modifier::BOLD),
    )))
}

/// A heavy bar over a pane's left border at `x`, beside the screen rows
/// `rows` of the cursor's entry, so the eye finds it from the edge of the pane
/// without anything in the list moving. Drawn after the border itself.
pub(crate) fn selected_row_edge(frame: &mut Frame, x: u16, rows: Range<u16>) {
    let edge = Style::default()
        .fg(theme::SELECTED_ROW_EDGE)
        .add_modifier(Modifier::BOLD);
    for y in rows {
        frame.buffer_mut()[(x, y)].set_symbol("┃").set_style(edge);
    }
}

/// Mark the panel that owns the keyboard with the grid's thick selection border.
pub fn focused_panel_block(chrome: &PanelChrome) -> Block<'static> {
    focused(titled_block(chrome, true), chrome)
}

/// The main interaction view's [`focused_panel_block`], whose agent, model,
/// effort and state ride the footer instead — see [`status_spans`] and
/// [`launch_spans`].
pub fn focused_interaction_block(chrome: &PanelChrome) -> Block<'static> {
    focused(titled_block(chrome, false), chrome)
}

fn focused(block: Block<'static>, chrome: &PanelChrome) -> Block<'static> {
    if !chrome.focused {
        return block;
    }
    block.border_type(BorderType::Thick).border_style(
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )
}

/// The interaction's state, `● idle 3s`, in its tone.
pub fn status_spans(chrome: &PanelChrome) -> Vec<Span<'static>> {
    let tone = match chrome.status_tone {
        StatusTone::Pending => theme::INFO,
        StatusTone::Running => theme::RUNNING_STATUS,
        StatusTone::Idle => theme::SUCCESS,
        StatusTone::Background => theme::MUTED_WARNING,
        StatusTone::Stopped(why) => why.color(),
        StatusTone::Ended => theme::INACTIVE,
        StatusTone::Error => theme::ERROR,
    };
    let mut spans = vec![
        Span::styled("● ", Style::default().fg(tone)),
        Span::styled(
            chrome.status.clone(),
            Style::default().fg(tone).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(elapsed) = &chrome.elapsed {
        spans.push(Span::styled(
            format!(" {elapsed}"),
            Style::default().fg(tone),
        ));
    }
    spans
}

/// The agent, model and effort the interaction runs with, `codex · gpt-5 ·
/// high`; a model or effort the agent has not reported yet is dimmed.
pub fn launch_spans(chrome: &PanelChrome) -> Vec<Span<'static>> {
    let text = Style::default().fg(theme::MUTED_TEXT);
    let value = |reported| {
        Style::default().fg(if reported {
            theme::TEXT
        } else {
            theme::ADDITIONAL_INFO
        })
    };
    let mut spans = vec![
        Span::styled(format!("{} · ", chrome.agent), text),
        Span::styled(chrome.model.clone(), value(chrome.model_reported)),
    ];
    if let Some(effort) = &chrome.effort {
        spans.push(Span::styled(" · ", text));
        spans.push(Span::styled(effort.clone(), value(chrome.effort_reported)));
    }
    spans
}

pub fn panel_block(chrome: &PanelChrome) -> Block<'static> {
    titled_block(chrome, true)
}

/// A panel block titled with where the interaction is, and with what it runs
/// with and its state when `launch_and_status`.
fn titled_block(chrome: &PanelChrome, launch_and_status: bool) -> Block<'static> {
    let text = Style::default().fg(theme::MUTED_TEXT);
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
    if let Some(parent) = &chrome.parent {
        // Points up to the interaction this one branched from.
        spans.push(Span::styled("↱ ", text));
        spans.push(Span::styled(
            parent.clone(),
            Style::default().fg(theme::CONVERSATION_CHANGE),
        ));
        spans.push(Span::styled(" · ", text));
    }
    // Named as the interactions list names it: `[worktree] → target`.
    if let Some(worktree) = &chrome.worktree {
        spans.push(Span::styled(
            format!("[{worktree}]"),
            Style::default().fg(theme::DIRECTORY_NAME),
        ));
        if let Some(into) = &chrome.into {
            spans.push(Span::styled(format!(" → {into}"), text));
        }
        spans.push(Span::styled(" · ", text));
    }
    if let Some(source) = &chrome.source_branch {
        spans.push(Span::styled(
            format!("from {source}"),
            Style::default().fg(theme::ACCENT),
        ));
        spans.push(Span::styled(" · ", text));
    }
    if launch_and_status {
        spans.extend(launch_spans(chrome));
        spans.push(Span::styled(" · ", text));
        spans.extend(status_spans(chrome));
    } else if spans.len() > 1 {
        // Drop the separator left after the last place.
        spans.pop();
    }
    let empty = spans.len() == 1 && chrome.suffix.is_none();
    let separator = if spans.len() > 1 { " · " } else { "" };
    spans.push(Span::styled(
        chrome
            .suffix
            .as_ref()
            .map(|suffix| format!("{separator}{suffix} "))
            .unwrap_or_else(|| " ".into()),
        text,
    ));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if chrome.focused {
            theme::ACCENT
        } else {
            theme::INACTIVE
        }));
    if empty {
        block
    } else {
        block.title(Line::from(spans))
    }
}

#[cfg(test)]
mod tests;
