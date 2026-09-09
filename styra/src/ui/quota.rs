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
use styra_server::{QuotaEvent, QuotaStatus};

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

/// The footer's warning badge: the provider whose plan is closest to refusing
/// work, named with how full that window is, or `None` while every window the
/// server has spoken about is still comfortable.
///
/// It reads the same log the view does rather than a second piece of state,
/// so the badge and the view can never disagree. A provider's plan has several
/// windows and each is read repeatedly, so only the newest reading per
/// provider-and-window counts: an older warning that has since been superseded
/// by a comfortable reading of the same window is not news, and a Claude
/// window says nothing about a Codex one.
///
/// A window whose reset has passed is dropped: it has turned over, so whatever
/// it said about being full is about a pool that no longer exists.
pub(crate) fn alert(app: &App, now_ms: u64) -> Option<Line<'static>> {
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
    let mut pressing: Vec<&QuotaEvent> = newest
        .into_iter()
        .filter(|reading| reading.status != QuotaStatus::Allowed)
        .filter(|reading| reading.resets_at_ms.is_none_or(|resets| resets > now_ms))
        .collect();
    // Worst first, and among equals the fuller window: the badge has room for
    // one window, so it has to be the one the operator would act on.
    pressing.sort_by(|left, right| {
        severity(right.status).cmp(&severity(left.status)).then(
            right
                .utilization
                .unwrap_or(0.0)
                .total_cmp(&left.utilization.unwrap_or(0.0)),
        )
    });
    let worst = pressing.first()?;
    let color = status_color(worst.status);
    let mut spans = vec![Span::styled(
        format!(
            " ⚠ {} {} {} ",
            worst.provider.as_str(),
            worst.window,
            worst.utilization_label()
        ),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )];
    // Only the worst window is spelled out; the rest are counted, so a second
    // provider filling up is still visible without crowding the footer.
    if pressing.len() > 1 {
        spans.push(Span::styled(
            format!("+{} ", pressing.len() - 1),
            Style::default().fg(color),
        ));
    }
    Some(Line::from(spans))
}

fn severity(status: QuotaStatus) -> u8 {
    match status {
        QuotaStatus::Allowed => 0,
        QuotaStatus::Warning => 1,
        QuotaStatus::Exhausted => 2,
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

    /// The badge is the whole point of the footer slot: an operator who never
    /// presses `Q` still has to learn that a plan is about to refuse them.
    #[test]
    fn a_pressing_window_shows_as_a_warning_badge_in_the_footer() {
        let mut app = app();
        assert!(!rendered(&app).contains('⚠'), "nothing to warn about yet");

        app.quota.replace(vec![
            reading("7d", QuotaStatus::Allowed, Some(0.1)),
            reading("five_hour", QuotaStatus::Warning, Some(0.91)),
        ]);
        let screen = rendered(&app);
        assert!(screen.contains("⚠ claude five_hour 91%"), "{screen}");
    }

    /// A window read again and found comfortable is not news; the badge has to
    /// follow the newest reading of each window rather than the worst one ever
    /// seen.
    #[test]
    fn a_superseded_warning_stops_showing() {
        let mut app = app();
        let mut later = reading("five_hour", QuotaStatus::Allowed, Some(0.02));
        later.at_ms = 2_000;
        app.quota.replace(vec![
            reading("five_hour", QuotaStatus::Warning, Some(0.91)),
            later,
        ]);

        assert!(alert(&app, 0).is_none());
    }

    /// Two providers can be under pressure at once, and only one fits: the
    /// worse one is named and the other counted.
    #[test]
    fn the_worst_window_is_named_and_the_rest_counted() {
        let mut app = app();
        let mut codex = reading("weekly", QuotaStatus::Exhausted, None);
        codex.provider = Provider::Codex;
        app.quota.replace(vec![
            reading("five_hour", QuotaStatus::Warning, Some(0.91)),
            codex,
        ]);

        let badge = alert(&app, 0).unwrap();
        assert!(badge.to_string().contains("⚠ codex weekly"), "{badge}");
        assert!(badge.to_string().contains("+1"), "{badge}");
    }

    /// A full window that has since reset describes a pool that no longer
    /// exists, so it must not keep the badge lit.
    #[test]
    fn a_window_whose_reset_has_passed_no_longer_warns() {
        let mut app = app();
        let mut exhausted = reading("five_hour", QuotaStatus::Exhausted, Some(1.0));
        exhausted.resets_at_ms = Some(10_000);
        app.quota.replace(vec![exhausted]);

        assert!(alert(&app, 9_000).is_some(), "still inside the window");
        assert!(alert(&app, 11_000).is_none(), "the window turned over");
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
