//! The quota view: what the providers have said about how much of the plan is
//! left.
//!
//! The readings come from the server, which reads them off every interaction's
//! wire and keeps a trimmed log of them in its store (see
//! `styra_server::quota`), so this view has something to show from the moment
//! a session is attached rather than only once a provider volunteers a figure.
//! They are account-wide *per provider* rather than per-session, so this view
//! shows every interaction's readings and names the provider, the session, and
//! the minute each came from — a stale 90% reading and a fresh one mean different
//! things, and a Claude window says nothing about a Codex one.

use super::{palette, render_placeholder, view_block};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_server::agent::Provider;
use styra_server::{QuotaEvent, QuotaStatus};

const FOOTER_WARNING_THRESHOLD: f64 = 0.75;
const FOOTER_ERROR_THRESHOLD: f64 = 0.90;

pub(crate) fn render_quota(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some("quota")).title_bottom(retry_title(app));

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
    let start = max_start.saturating_sub(app.quota.scroll_back() as usize) as u16;
    let paragraph = Paragraph::new(lines).block(block).scroll((start, 0));
    frame.render_widget(paragraph, area);
}

/// The bottom border's note on what happens to this interaction if a plan
/// window refuses it: nothing, or the server picking it up again once the
/// window turns over.
///
/// It rides this view's border rather than the shared chrome because this is
/// where `R` sets it and where the rejection that stopped a session is on the
/// record, with the minute its window resets beside it. An operator who has
/// just been cut off comes here to see the limit; the answer to "and will it
/// pick itself back up" belongs in the same glance.
fn retry_title(app: &App) -> Line<'static> {
    let (text, color) = if app.auto_retry {
        (" R rate-limit retry: on ", palette::SUCCESS)
    } else {
        (" R rate-limit retry: off ", palette::INACTIVE)
    };
    Line::from(Span::styled(
        text,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

/// One reading: when it was seen, how full it is, whose plan and which window,
/// when it resets, and where it was seen. The percentage leads, since that is
/// what the view is consulted for, with the reading's own time ahead of it so
/// a column of readings reads as a timeline.
fn quota_line(reading: &QuotaEvent) -> Line<'static> {
    let color = status_color(reading.status);
    let mut spans = vec![
        Span::styled(
            format!("{} ", clock(reading.at_ms)),
            Style::default().fg(palette::MUTED_TEXT),
        ),
        Span::styled(
            format!("{:>5} ", reading.utilization_label()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{:<10} ", reading.provider.as_str()),
            Style::default().fg(palette::TEXT),
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

/// The latest provider percentages for the footer. Codex reports both its
/// concrete windows; Claude reports `five_hour` and deliberately withholds the
/// percentage when clear, which is displayed as `-`. Generic `plan` refusals
/// are omitted because they have no percentage.
pub(crate) fn alert(app: &App) -> Option<Line<'static>> {
    let mut newest: Vec<&QuotaEvent> = Vec::new();
    for reading in app.quota.iter() {
        match newest
            .iter_mut()
            .find(|kept| kept.provider == reading.provider && kept.window == reading.window)
        {
            Some(kept) if kept.at_ms <= reading.at_ms => *kept = reading,
            Some(_) => {}
            None => newest.push(reading),
        }
    }
    let limits: Vec<&QuotaEvent> = newest
        .iter()
        .copied()
        .filter(|reading| {
            reading.provider == Provider::Codex
                && matches!(reading.window.as_str(), "5h" | "7d")
                && reading.utilization.is_some()
        })
        .collect();
    let claude = newest
        .iter()
        .copied()
        .find(|reading| reading.provider == Provider::Claude && reading.window == "five_hour");
    if limits.is_empty() && claude.is_none() {
        return None;
    }
    let mut spans = Vec::new();
    if !limits.is_empty() {
        spans.push(Span::styled(
            " codex: ",
            Style::default().fg(palette::MUTED_TEXT),
        ));
    }
    let mut wrote_percentage = false;
    for window in ["5h", "7d"] {
        if let Some(reading) = limits.iter().find(|reading| reading.window == window) {
            let separator = wrote_percentage.then_some("/").unwrap_or_default();
            spans.push(Span::styled(
                format!("{separator}{}", reading.utilization_label()),
                Style::default()
                    .fg(footer_color(reading.utilization.expect("filtered above")))
                    .add_modifier(Modifier::BOLD),
            ));
            wrote_percentage = true;
        }
    }
    if let Some(reading) = claude {
        spans.push(Span::styled(
            " claude: ",
            Style::default().fg(palette::MUTED_TEXT),
        ));
        let (label, color) = match reading.utilization {
            Some(utilization) => (reading.utilization_label(), footer_color(utilization)),
            // Claude only omits this figure on an `allowed` event; that is a
            // fresh confirmation that it is not close to warning, not an
            // unknown amount to carry over from a previous event.
            None if reading.status == QuotaStatus::Allowed => ("-".into(), palette::MUTED_TEXT),
            None => ("?".into(), status_color(reading.status)),
        };
        spans.push(Span::styled(
            label,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
    }
    Some(Line::from(spans))
}

fn footer_color(utilization: f64) -> Color {
    if utilization > FOOTER_ERROR_THRESHOLD {
        palette::ERROR
    } else if utilization > FOOTER_WARNING_THRESHOLD {
        palette::WARNING
    } else {
        palette::MUTED_TEXT
    }
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

/// A wall-clock `HH:MM` in the operator's own zone. Both times this shows —
/// when a reading was taken, and when its window resets — are read against the
/// clock on the wall, so they are rendered in the zone that clock is in.
fn clock(at_ms: u64) -> String {
    minute_of_day(at_ms, local_offset_seconds(at_ms))
}

/// The minute `at_ms` falls on, `offset_seconds` east of UTC.
fn minute_of_day(at_ms: u64, offset_seconds: i64) -> String {
    let minutes = (at_ms / 60_000) as i64 + offset_seconds / 60;
    let minute_of_day = minutes.rem_euclid(1_440);
    format!("{:02}:{:02}", minute_of_day / 60, minute_of_day % 60)
}

/// How far the operator's zone is from UTC at that moment, in seconds — the
/// moment matters, since a zone's offset moves across a daylight-saving
/// boundary.
///
/// The lookup goes through the C library rather than a date crate: the zone
/// rules live in the system's tzdata either way, and `localtime_r` reads them
/// (and `TZ`) exactly as every other tool on the operator's machine does. A
/// machine whose zone cannot be resolved gets UTC, which is what the view
/// showed before it knew how to ask. Both glibc and musl load the zone on the
/// first conversion, so no separate `tzset` is needed.
fn local_offset_seconds(at_ms: u64) -> i64 {
    let seconds = (at_ms / 1_000) as libc::time_t;
    let mut broken_down: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: `seconds` is a valid `time_t` and `broken_down` a valid, owned
    // `tm` that outlives the call; `localtime_r` writes only into it and
    // returns null rather than touching it when the time cannot be converted.
    let converted = unsafe { libc::localtime_r(&seconds, &mut broken_down) };
    if converted.is_null() {
        return 0;
    }
    broken_down.tm_gmtoff as i64
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use crate::app::View;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use styra_server::protocol::Provider;

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
        testing::app("s1")
    }

    fn reading(window: &str, status: QuotaStatus, utilization: Option<f64>) -> QuotaEvent {
        QuotaEvent {
            at_ms: 1_000,
            session_id: "s1".into(),
            provider: Provider::Claude,
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
        app.quota.replace(vec![
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

    /// The log mixes both accounts' readings, so each row has to say whose
    /// plan it measures and when it was taken.
    #[test]
    fn each_reading_names_its_provider_and_the_minute_it_was_seen() {
        let mut app = app();
        let mut codex = reading("7d", QuotaStatus::Allowed, Some(0.4));
        codex.provider = Provider::Codex;
        codex.at_ms = 1_788_290_400_000;
        app.quota.replace(vec![
            reading("five_hour", QuotaStatus::Warning, Some(0.91)),
            codex,
        ]);
        app.toggle_view(View::Quota);
        let screen = rendered(&app);
        assert!(screen.contains("claude"));
        assert!(screen.contains("codex"));
        assert!(screen.contains(&clock(1_788_290_400_000)));
    }

    /// A permitted Claude reading genuinely carries no figure; the view has to
    /// say so rather than show a misleading 0%.
    #[test]
    fn a_reading_without_a_usage_figure_shows_no_percentage() {
        let mut app = app();
        app.quota
            .replace(vec![reading("five_hour", QuotaStatus::Allowed, None)]);
        app.toggle_view(View::Quota);
        let screen = rendered(&app);
        assert!(screen.contains("?"));
        assert!(!screen.contains("0%"));
    }

    /// `R` is pressed in this view, so this view has to say which way it is
    /// set — including before any reading has arrived to set it over.
    #[test]
    fn the_quota_view_says_whether_a_refused_session_would_be_asked_again() {
        let mut app = app();
        app.toggle_view(View::Quota);
        assert!(rendered(&app).contains("R rate-limit retry: off"));

        // The server's answer, adopted when the interaction was attached.
        app.auto_retry = true;

        assert!(rendered(&app).contains("R rate-limit retry: on"));
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
        assert_eq!(app.quota.iter().count(), 1);
        assert_eq!(app.notices.len(), 1);
        assert!(app
            .notices
            .iter()
            .any(|notice| notice.text.contains("91% used")));
        assert_eq!(app.log.iter().count(), 1);
        assert_eq!(
            app.log.newest().unwrap().level,
            styra_server::LogLevel::Warn
        );
    }

    #[test]
    fn an_exhausted_window_is_logged_as_an_error() {
        let mut app = app();
        app.note_quota(reading("five_hour", QuotaStatus::Exhausted, None));
        assert_eq!(
            app.log.newest().unwrap().level,
            styra_server::LogLevel::Error
        );
        assert!(app.log.newest().unwrap().message.contains("exhausted"));
    }

    /// The footer carries the latest two concrete Codex limits, including a
    /// comfortable one: it is a live readout rather than a warning badge.
    #[test]
    fn the_footer_shows_both_latest_codex_windows() {
        let mut app = app();
        let mut primary = reading("5h", QuotaStatus::Allowed, Some(0.12));
        primary.provider = Provider::Codex;
        let mut weekly = reading("7d", QuotaStatus::Allowed, Some(0.81));
        weekly.provider = Provider::Codex;
        app.quota.replace(vec![primary, weekly]);

        let footer = alert(&app).unwrap();
        assert_eq!(footer.to_string(), " codex: 12%/81%");
    }

    /// Claude omits the percentage in an allowed `five_hour` event. That is
    /// an explicit clear, so it replaces an earlier warning with `-`.
    #[test]
    fn a_clear_claude_event_shows_a_dash_in_the_footer() {
        let mut app = app();
        let mut warning = reading("five_hour", QuotaStatus::Warning, Some(0.91));
        warning.at_ms = 1_000;
        let mut clear = reading("five_hour", QuotaStatus::Allowed, None);
        clear.at_ms = 2_000;
        app.quota.replace(vec![warning, clear]);

        assert_eq!(alert(&app).unwrap().to_string(), " claude: -");
    }

    /// Codex does not name the window when it refuses a turn, so a later
    /// successful concrete-window update has to clear that catch-all `plan`
    /// refusal too.
    #[test]
    fn a_later_codex_window_update_clears_a_stale_plan_refusal() {
        let mut app = app();
        let mut refused = reading("plan", QuotaStatus::Exhausted, None);
        refused.provider = Provider::Codex;
        let mut primary = reading("5h", QuotaStatus::Allowed, Some(0.12));
        primary.provider = Provider::Codex;
        primary.at_ms = 2_000;
        let mut weekly = reading("7d", QuotaStatus::Allowed, Some(0.81));
        weekly.provider = Provider::Codex;
        weekly.at_ms = 2_000;
        app.quota.replace(vec![refused, primary, weekly]);

        assert_eq!(alert(&app).unwrap().to_string(), " codex: 12%/81%");
    }

    /// Yellow starts strictly above 75%, and red strictly above 90%, using the
    /// actual fraction rather than its rounded display label.
    #[test]
    fn footer_colours_each_window_from_its_percentage() {
        let mut app = app();
        let mut primary = reading("5h", QuotaStatus::Allowed, Some(0.76));
        primary.provider = Provider::Codex;
        let mut weekly = reading("7d", QuotaStatus::Warning, Some(0.91));
        weekly.provider = Provider::Codex;
        app.quota.replace(vec![primary, weekly]);

        let footer = alert(&app).unwrap();
        assert_eq!(footer.spans[1].style.fg, Some(palette::WARNING));
        assert_eq!(footer.spans[2].style.fg, Some(palette::ERROR));
    }

    /// Times are shown on the operator's own clock, so the minute a moment
    /// lands on depends on the zone the machine is in.
    #[test]
    fn times_render_as_a_wall_clock_minute_in_the_operators_zone() {
        // A real five-hour reset, at 19:20 UTC.
        let reset = 1_788_290_400_000;
        assert_eq!(minute_of_day(reset, 0), "19:20");
        // An hour east, and India's half-hour offset.
        assert_eq!(minute_of_day(reset, 3_600), "20:20");
        assert_eq!(minute_of_day(reset, 5 * 3_600 + 1_800), "00:50");
        // West far enough to fall back into the previous day.
        assert_eq!(minute_of_day(reset, -8 * 3_600), "11:20");
        assert_eq!(minute_of_day(0, -3_600), "23:00");
    }

    /// The offset has to come from the machine's own zone rather than be
    /// assumed: `TZ` is what every other tool on it obeys.
    #[test]
    fn the_offset_is_the_machines_own() {
        // Whatever this machine's zone is, a rendered clock has to agree with
        // the offset the C library reports for that same moment.
        let at_ms = 1_788_290_400_000;
        assert_eq!(
            clock(at_ms),
            minute_of_day(at_ms, local_offset_seconds(at_ms))
        );
    }
}
