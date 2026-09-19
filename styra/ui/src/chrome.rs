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
    Stopped,
    Error,
    Ended,
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

pub fn panel_block(chrome: &PanelChrome) -> Block<'static> {
    let tone = match chrome.status_tone {
        StatusTone::Pending => palette::INFO,
        StatusTone::Running => palette::WARNING,
        StatusTone::Idle => palette::SUCCESS,
        StatusTone::Background => palette::MUTED_WARNING,
        StatusTone::Stopped | StatusTone::Ended => palette::INACTIVE,
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
