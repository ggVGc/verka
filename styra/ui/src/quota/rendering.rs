use super::*;
use crate::chrome::panel_block;
use crate::theme;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use styra_protocol::{QuotaEvent, QuotaStatus};

/// How many past readings the log shows when the panel is tall enough.
const LOG_LINES: usize = 10;

pub fn render(frame: &mut Frame, view: &QuotaView<'_>, area: Rect) {
    let (retry, color) = if view.auto_retry {
        (" R rate-limit retry: on ", theme::SUCCESS)
    } else {
        (" R rate-limit retry: off ", theme::INACTIVE)
    };
    let block = panel_block(&view.chrome).title_bottom(Line::from(Span::styled(
        retry,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )));
    if view.readings.is_empty() {
        frame.render_widget(
            Paragraph::new("  no quota readings yet — press Q to ask the server").block(block),
            area,
        );
        return;
    }
    let mut lines = summary_lines(view.readings, view.now_ms);
    let height = area.height.saturating_sub(2) as usize;
    let room = height.saturating_sub(lines.len() + 1).min(LOG_LINES);
    if room > 0 {
        lines.push(Line::from(Span::styled(
            "  recent readings",
            Style::default().fg(theme::INACTIVE),
        )));
        let log = view
            .readings
            .iter()
            .map(|reading| line(reading, view.now_ms))
            .collect::<Vec<_>>();
        let end = log.len().saturating_sub(view.scroll_back as usize).max(1);
        lines.extend(log[end.saturating_sub(room)..end].iter().cloned());
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// The current state of every window a provider has reported, newest reading
/// per window, so the panel opens on where the quotas stand rather than on
/// history.
fn summary_lines(readings: &[&QuotaEvent], now_ms: u64) -> Vec<Line<'static>> {
    let mut current = newest_per_window(readings);
    current.retain(|reading| !superseded(reading, readings));
    current.sort_by(|left, right| {
        left.provider
            .as_str()
            .cmp(right.provider.as_str())
            .then_with(|| left.window.cmp(&right.window))
    });
    current
        .into_iter()
        .map(|reading| {
            let color = status_color(reading.status);
            let mut spans = vec![
                Span::styled(
                    format!("  {:<8}", reading.provider.as_str()),
                    Style::default()
                        .fg(theme::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<10} ", reading.window),
                    Style::default().fg(theme::MUTED_TEXT),
                ),
                Span::styled(
                    format!("{:>5} ", reading.utilization_label()),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<10}", status_label(reading.status)),
                    Style::default().fg(color),
                ),
            ];
            spans.push(Span::styled(
                match reading.resets_at_ms {
                    Some(reset) if reset <= now_ms => "reset due".into(),
                    Some(reset) => format!("resets {}", stamp(reset, now_ms)),
                    None => "reset unknown".into(),
                },
                Style::default().fg(theme::MUTED_TEXT),
            ));
            Line::from(spans)
        })
        .collect()
}

fn line(reading: &QuotaEvent, now_ms: u64) -> Line<'static> {
    let color = status_color(reading.status);
    let mut spans = vec![
        Span::styled(
            format!("{} ", clock(reading.at_ms)),
            Style::default().fg(theme::MUTED_TEXT),
        ),
        Span::styled(
            format!("{:>5} ", reading.utilization_label()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10} ", reading.provider.as_str()),
            Style::default().fg(theme::TEXT),
        ),
        Span::styled(
            format!("{:<10} ", reading.window),
            Style::default().fg(theme::TEXT),
        ),
        Span::styled(
            format!("{:<9} ", status_label(reading.status)),
            Style::default().fg(color),
        ),
    ];
    if let Some(reset) = reading.resets_at_ms {
        spans.push(Span::styled(
            format!("resets {} ", stamp(reset, now_ms)),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    if let Some(detail) = &reading.detail {
        spans.push(Span::styled(
            format!("{detail} "),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    }
    spans.push(Span::styled(
        format!("· {}", reading.session_id),
        Style::default().fg(theme::INACTIVE),
    ));
    Line::from(spans)
}

fn status_color(status: QuotaStatus) -> Color {
    match status {
        QuotaStatus::Allowed => theme::MUTED_TEXT,
        QuotaStatus::Warning => theme::QUOTA_WARNING,
        QuotaStatus::Exhausted => theme::ERROR,
    }
}
fn status_label(status: QuotaStatus) -> &'static str {
    match status {
        QuotaStatus::Allowed => "ok",
        QuotaStatus::Warning => "warning",
        QuotaStatus::Exhausted => "exhausted",
    }
}
#[cfg(test)]
mod tests;
