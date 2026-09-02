//! The quota view: what the providers have said about how much of the plan is
//! left.
//!
//! The readings come from the server, which reads them off every interaction's
//! wire and keeps them in memory (see `styra_server::quota`). They are
//! account-wide rather than per-session, so this view shows every interaction's
//! readings and names the session each came from.

use super::{palette, render_placeholder, view_block};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_server::{QuotaEvent, QuotaStatus};

pub(crate) fn render_quota(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some("quota"));

    if app.quota.is_empty() {
        render_placeholder(
            frame,
            block,
            area,
            "  no quota readings yet — press Q to ask the server",
        );
        return;
    }

    let lines: Vec<Line<'static>> = app.quota.iter().map(quota_line).collect();
    let viewport = area.height.saturating_sub(2) as usize;
    let max_start = lines.len().saturating_sub(viewport);
    let start = max_start.saturating_sub(app.quota_scroll_back as usize) as u16;
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
}

/// One reading: its window, how full it is, when it resets, and where it was
/// seen. The percentage leads, since that is what the view is consulted for.
fn quota_line(reading: &QuotaEvent) -> Line<'static> {
    let color = status_color(reading.status);
    let mut spans = vec![
        Span::styled(
            format!("{:>5} ", reading.utilization_label()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10} ", reading.window),
            Style::default().fg(palette::TEXT),
        ),
        Span::styled(
            format!("{:<9} ", status_label(reading.status)),
            Style::default().fg(color),
        ),
    ];
    if let Some(resets_at_ms) = reading.resets_at_ms {
        spans.push(Span::styled(
            format!("resets {} ", clock(resets_at_ms)),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    }
    if let Some(detail) = &reading.detail {
        spans.push(Span::styled(
            format!("{detail} "),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    }
    spans.push(Span::styled(
        format!("· {}", reading.session_id),
        Style::default().fg(palette::INACTIVE),
    ));
    Line::from(spans)
}

pub(crate) fn status_color(status: QuotaStatus) -> Color {
    match status {
        QuotaStatus::Allowed => palette::MUTED_TEXT,
        QuotaStatus::Warning => palette::WARNING,
        QuotaStatus::Exhausted => palette::ERROR,
    }
}

fn status_label(status: QuotaStatus) -> &'static str {
    match status {
        QuotaStatus::Allowed => "ok",
        QuotaStatus::Warning => "warning",
        QuotaStatus::Exhausted => "exhausted",
    }
}

/// A wall-clock `HH:MM` in UTC. The reset only matters to the minute, and
/// deriving it here keeps the view free of a date-formatting dependency.
fn clock(at_ms: u64) -> String {
    let minutes_of_day = (at_ms / 60_000) % 1_440;
    format!("{:02}:{:02}Z", minutes_of_day / 60, minutes_of_day % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::View;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .clone()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    fn app() -> App {
        App::new(
            styra_server::agent::Selection::parse("codex").unwrap(),
            "s1",
        )
    }

    fn reading(window: &str, status: QuotaStatus, utilization: Option<f64>) -> QuotaEvent {
        QuotaEvent {
            at_ms: 1_000,
            session_id: "s1".into(),
            window: window.into(),
            status,
            utilization,
            resets_at_ms: None,
            detail: None,
        }
    }

    #[test]
    fn the_quota_view_lists_each_window_with_its_usage() {
        let mut app = app();
        app.set_quota(vec![
            reading("five_hour", QuotaStatus::Warning, Some(0.91)),
            reading("7d", QuotaStatus::Allowed, Some(0.125)),
        ]);
        app.toggle_view(View::Quota);
        let screen = rendered(&app);
        assert!(screen.contains("quota"));
        assert!(screen.contains("five_hour"));
        assert!(screen.contains("91%"));
        assert!(screen.contains("warning"));
        // 12.5% renders as 12: `{:.0}` rounds half to even.
        assert!(screen.contains("12%"));
    }

    /// A permitted Claude reading genuinely carries no figure; the view has to
    /// say so rather than show a misleading 0%.
    #[test]
    fn a_reading_without_a_usage_figure_shows_no_percentage() {
        let mut app = app();
        app.set_quota(vec![reading("five_hour", QuotaStatus::Allowed, None)]);
        app.toggle_view(View::Quota);
        let screen = rendered(&app);
        assert!(screen.contains("?"));
        assert!(!screen.contains("0%"));
    }

    #[test]
    fn an_empty_quota_view_says_how_to_fill_it() {
        let mut app = app();
        app.toggle_view(View::Quota);
        assert!(rendered(&app).contains("no quota readings yet"));
    }

    /// An announced reading has to be visible without asking the server, and
    /// has to reach an operator who is not looking at the quota view.
    #[test]
    fn an_announced_reading_shows_in_the_view_the_log_and_a_notice() {
        let mut app = app();
        app.note_quota(reading("five_hour", QuotaStatus::Warning, Some(0.91)));
        assert_eq!(app.quota.len(), 1);
        assert_eq!(app.action_messages.len(), 1);
        assert!(app.action_messages[0].text.contains("91% used"));
        assert_eq!(app.log.len(), 1);
        assert_eq!(app.log[0].level, styra_server::LogLevel::Warn);
    }

    #[test]
    fn an_exhausted_window_is_logged_as_an_error() {
        let mut app = app();
        app.note_quota(reading("five_hour", QuotaStatus::Exhausted, None));
        assert_eq!(app.log[0].level, styra_server::LogLevel::Error);
        assert!(app.log[0].message.contains("exhausted"));
    }

    #[test]
    fn reset_times_render_as_a_wall_clock_minute() {
        assert_eq!(clock(0), "00:00Z");
        // A real five-hour reset, as UTC rather than whatever the operator's
        // zone happens to be — the suffix says which.
        assert_eq!(clock(1_788_290_400_000), "19:20Z");
    }
}
