//! Provider quota presentation and compact footer readings.

use crate::chrome::{panel_block, PanelChrome};
use crate::footer::{Segment, Tone};
use crate::palette;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};
use styra_protocol::{agent::Provider, QuotaEvent, QuotaStatus};

const WARNING: f64 = 0.75;
const ERROR: f64 = 0.90;

/// How many past readings the log shows when the panel is tall enough.
const LOG_LINES: usize = 10;

pub struct QuotaView<'a> {
    pub chrome: PanelChrome,
    pub readings: &'a [&'a QuotaEvent],
    pub auto_retry: bool,
    pub scroll_back: u16,
    /// Time is supplied by the adapter so fixtures and mocks stay deterministic.
    pub now_ms: u64,
}

pub fn render(frame: &mut Frame, view: &QuotaView<'_>, area: Rect) {
    let (retry, color) = if view.auto_retry {
        (" R rate-limit retry: on ", palette::SUCCESS)
    } else {
        (" R rate-limit retry: off ", palette::INACTIVE)
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
            Style::default().fg(palette::INACTIVE),
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
                        .fg(palette::TEXT)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:<10} ", reading.window),
                    Style::default().fg(palette::MUTED_TEXT),
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
                Style::default().fg(palette::MUTED_TEXT),
            ));
            Line::from(spans)
        })
        .collect()
}

/// Whether a spent window has since been contradicted by the provider.
///
/// A refusal is filed under a window of its own (`plan`), which no later
/// success ever reports on, so nothing supersedes it by window and the summary
/// would call the plan spent for good. But a reading only exists because a turn
/// ran and the provider answered it, so any later permitted or filling reading
/// from the same provider is that provider saying it is serving again — the
/// refusal is then history, and history is what the log below is for.
fn superseded(reading: &QuotaEvent, readings: &[&QuotaEvent]) -> bool {
    reading.status == QuotaStatus::Exhausted
        && readings.iter().any(|later| {
            later.provider == reading.provider
                && later.status != QuotaStatus::Exhausted
                && later.at_ms > reading.at_ms
        })
}

/// The latest reading for each provider-and-window pair, in first-seen order.
fn newest_per_window<'a>(readings: &[&'a QuotaEvent]) -> Vec<&'a QuotaEvent> {
    let mut newest: Vec<&QuotaEvent> = Vec::new();
    for reading in readings {
        match newest
            .iter_mut()
            .find(|kept| kept.provider == reading.provider && kept.window == reading.window)
        {
            Some(kept) if kept.at_ms <= reading.at_ms => *kept = *reading,
            Some(_) => {}
            None => newest.push(*reading),
        }
    }
    newest
}

fn line(reading: &QuotaEvent, now_ms: u64) -> Line<'static> {
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
    if let Some(reset) = reading.resets_at_ms {
        spans.push(Span::styled(
            format!("resets {} ", stamp(reset, now_ms)),
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

/// Time is supplied by the adapter so fixtures and mocks stay deterministic.
pub fn footer_segments(readings: &[&QuotaEvent], now_ms: u64) -> Vec<Segment> {
    let newest = newest_per_window(readings);
    let codex = newest
        .iter()
        .copied()
        .filter(|reading| {
            reading.provider == Provider::Codex
                && matches!(reading.window.as_str(), "5h" | "7d")
                && reading.utilization.is_some()
        })
        .collect::<Vec<_>>();
    let claude = newest
        .iter()
        .copied()
        .find(|reading| reading.provider == Provider::Claude && reading.window == "five_hour");
    let spent = spent_label(&newest, readings, Provider::Codex, now_ms);
    if codex.is_empty() && spent.is_none() && claude.is_none() {
        return Vec::new();
    }
    let mut output = Vec::new();
    if !codex.is_empty() || spent.is_some() {
        output.push(segment(" codex: ", Tone::Muted, false));
    }
    // A spent plan replaces the percentages rather than joining them: the
    // figures say how full the windows were on the last turn that ran, and the
    // one thing worth the footer's width once work is being refused is when it
    // will be taken again.
    if let Some(spent) = spent {
        output.push(segment(spent, Tone::Error, true));
    } else {
        let mut wrote = false;
        for window in ["5h", "7d"] {
            if let Some(reading) = codex.iter().find(|reading| reading.window == window) {
                output.push(segment(
                    format!(
                        "{}{}",
                        if wrote { "/" } else { "" },
                        reading.utilization_label()
                    ),
                    utilization_tone(reading.utilization.expect("filtered")),
                    true,
                ));
                wrote = true;
            }
        }
    }
    if let Some(reading) = claude {
        output.push(segment(" claude: ", Tone::Muted, false));
        let (label, tone) = match (reading.status, reading.utilization, reading.resets_at_ms) {
            (QuotaStatus::Exhausted, _, Some(reset)) if reset <= now_ms => {
                ("-".into(), Tone::Muted)
            }
            (QuotaStatus::Exhausted, _, Some(reset)) => {
                (format!("resets {}", clock(reset)), Tone::Error)
            }
            (_, Some(value), _) => (reading.utilization_label(), utilization_tone(value)),
            (QuotaStatus::Allowed, None, _) => ("-".into(), Tone::Muted),
            (QuotaStatus::Warning, None, _) => ("?".into(), Tone::Warning),
            (QuotaStatus::Exhausted, None, _) => ("?".into(), Tone::Error),
        };
        output.push(segment(label, tone, true));
    }
    output
}

/// What the footer says about a provider whose plan is refusing work: when it
/// comes back, or that it is spent for a reason no clock answers.
///
/// Codex reports its exhaustion in pieces. The refusal itself is filed under
/// `plan` and names no reset — it states one only in prose — while the window
/// that filled up is a reading of its own, carrying the moment it turns over.
/// Both are looked at, and the soonest reset any of them names is the one
/// quoted, because that is when the provider next takes work.
///
/// `None` when nothing is standing in refusal: no window has refused, a later
/// reading says the provider is serving again, or the moment it named has
/// already passed — which is as good as serving until it refuses again, and
/// leaves the footer showing the figures instead of a stale reset.
fn spent_label(
    newest: &[&QuotaEvent],
    readings: &[&QuotaEvent],
    provider: Provider,
    now_ms: u64,
) -> Option<String> {
    let spent = newest
        .iter()
        .copied()
        .filter(|reading| {
            reading.provider == provider
                && reading.status == QuotaStatus::Exhausted
                && !superseded(reading, readings)
        })
        .collect::<Vec<_>>();
    if spent.is_empty() {
        return None;
    }
    match spent
        .iter()
        .filter_map(|reading| reading.resets_at_ms)
        .min()
    {
        Some(reset) if reset <= now_ms => None,
        Some(reset) => Some(format!("resets {}", stamp(reset, now_ms))),
        // A refusal that names no moment is still worth the row: an account out
        // of credits waits on somebody topping it up, and a footer showing the
        // last percentages would read as a plan with room to spare.
        None => Some("spent".into()),
    }
}

fn segment(text: impl Into<String>, tone: Tone, bold: bool) -> Segment {
    Segment {
        text: text.into(),
        tone,
        bold,
    }
}
fn utilization_tone(value: f64) -> Tone {
    if value > ERROR {
        Tone::Error
    } else if value > WARNING {
        Tone::Warning
    } else {
        Tone::Muted
    }
}
fn status_color(status: QuotaStatus) -> Color {
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
fn clock(at_ms: u64) -> String {
    minute_of_day(at_ms, local_offset_seconds(at_ms))
}
/// A reset on another day needs its date to be read at all, so a time alone is
/// only enough while the moment shares `now_ms`'s local day.
fn stamp(at_ms: u64, now_ms: u64) -> String {
    let offset = local_offset_seconds(at_ms);
    if local_day(at_ms, offset) == local_day(now_ms, local_offset_seconds(now_ms)) {
        minute_of_day(at_ms, offset)
    } else {
        format!(
            "{} {}",
            civil_date(local_day(at_ms, offset)),
            minute_of_day(at_ms, offset)
        )
    }
}
fn local_day(at_ms: u64, offset: i64) -> i64 {
    ((at_ms / 1_000) as i64 + offset).div_euclid(24 * 60 * 60)
}
/// Days since the epoch to `YYYY-MM-DD`, by Howard Hinnant's civil-from-days.
fn civil_date(days: i64) -> String {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}
fn minute_of_day(at_ms: u64, offset: i64) -> String {
    let minute = ((at_ms / 1_000) as i64 + offset)
        .div_euclid(60)
        .rem_euclid(24 * 60);
    format!("{:02}:{:02}", minute / 60, minute % 60)
}
fn local_offset_seconds(at_ms: u64) -> i64 {
    let seconds = (at_ms / 1_000) as libc::time_t;
    let mut value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for this call and `value` is owned.
    if unsafe { libc::localtime_r(&seconds, &mut value) }.is_null() {
        0
    } else {
        value.tm_gmtoff as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::StatusTone;
    use ratatui::{backend::TestBackend, Terminal};
    fn reading(
        provider: Provider,
        window: &str,
        status: QuotaStatus,
        utilization: Option<f64>,
    ) -> QuotaEvent {
        QuotaEvent {
            at_ms: 1_000,
            session_id: "s1".into(),
            provider,
            window: window.into(),
            status,
            utilization,
            resets_at_ms: None,
            detail: None,
        }
    }
    fn text(segments: &[Segment]) -> String {
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }
    fn chrome() -> PanelChrome {
        PanelChrome {
            focused: true,
            workspace: None,
            agent: "codex".into(),
            model: "gpt".into(),
            model_reported: true,
            effort: None,
            effort_reported: false,
            status: "idle".into(),
            status_tone: StatusTone::Idle,
            elapsed: None,
            suffix: Some("quota".into()),
            session: None,
        }
    }
    #[test]
    fn view_renders_reading_and_retry_state() {
        let reading = reading(Provider::Codex, "5h", QuotaStatus::Warning, Some(0.81));
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &QuotaView {
                        chrome: chrome(),
                        readings: &[&reading],
                        auto_retry: true,
                        scroll_back: 0,
                        now_ms: 1_000,
                    },
                    frame.area(),
                )
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        for expected in ["81%", "codex", "5h", "rate-limit retry: on"] {
            assert!(output.contains(expected), "{output}");
        }
    }
    #[test]
    fn view_summarises_current_state_with_dated_future_resets() {
        let mut old = reading(Provider::Codex, "7d", QuotaStatus::Allowed, Some(0.10));
        old.at_ms = 1_000;
        let mut weekly = reading(Provider::Codex, "7d", QuotaStatus::Warning, Some(0.91));
        weekly.at_ms = 2_000;
        weekly.resets_at_ms = Some(2_000 + 7 * 24 * 3_600 * 1_000);
        let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &QuotaView {
                        chrome: chrome(),
                        readings: &[&old, &weekly],
                        auto_retry: true,
                        scroll_back: 0,
                        now_ms: 2_000,
                    },
                    frame.area(),
                )
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        // One summary row for the window, showing the newest reading only.
        assert_eq!(output.matches("7d").count(), 3, "{output}");
        assert!(output.contains("resets 1970-01-0"), "{output}");
        assert!(output.contains("recent readings"), "{output}");
    }
    #[test]
    fn a_later_serving_reading_retires_a_spent_plan_from_the_summary() {
        let mut refusal = reading(Provider::Codex, "plan", QuotaStatus::Exhausted, None);
        refusal.at_ms = 1_000;
        let mut served = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.04));
        served.at_ms = 2_000;
        let summary = summary_lines(&[&refusal, &served], 2_000)
            .iter()
            .map(|line| line.to_string())
            .collect::<String>();
        assert!(!summary.contains("exhausted"), "{summary}");
        assert!(summary.contains("5h"), "{summary}");
        // Until the provider serves again the refusal is the standing fact.
        let still = summary_lines(&[&refusal], 2_000)
            .iter()
            .map(|line| line.to_string())
            .collect::<String>();
        assert!(still.contains("exhausted"), "{still}");
        // Another provider's success says nothing about this one.
        let mut elsewhere = reading(Provider::Claude, "five_hour", QuotaStatus::Allowed, None);
        elsewhere.at_ms = 3_000;
        let across = summary_lines(&[&refusal, &elsewhere], 3_000)
            .iter()
            .map(|line| line.to_string())
            .collect::<String>();
        assert!(across.contains("exhausted"), "{across}");
    }
    #[test]
    fn log_shows_only_what_fits_below_the_summary() {
        let readings = (0..40)
            .map(|index| {
                let mut reading = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.10));
                reading.detail = Some(format!("entry{index}"));
                reading
            })
            .collect::<Vec<_>>();
        let borrowed = readings.iter().collect::<Vec<_>>();
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &QuotaView {
                        chrome: chrome(),
                        readings: &borrowed,
                        auto_retry: false,
                        scroll_back: 0,
                        now_ms: 1_000,
                    },
                    frame.area(),
                )
            })
            .unwrap();
        let output = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(output.contains("entry39"), "{output}");
        assert!(output.contains("entry30"), "{output}");
        assert!(!output.contains("entry29"), "{output}");
    }
    #[test]
    fn stamp_dates_only_moments_outside_the_current_day() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(20_352), "2025-09-21");
        assert_eq!(
            stamp(1_000, 2_000),
            minute_of_day(1_000, local_offset_seconds(1_000))
        );
        assert!(stamp(8 * 24 * 3_600 * 1_000, 1_000).contains("1970-01-0"));
    }
    #[test]
    fn footer_keeps_concrete_codex_windows_and_tones() {
        let primary = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.12));
        let weekly = reading(Provider::Codex, "7d", QuotaStatus::Warning, Some(0.91));
        let segments = footer_segments(&[&primary, &weekly], 0);
        assert_eq!(text(&segments), " codex: 12%/91%");
        assert_eq!(segments[2].tone, Tone::Error);
    }
    /// The refusal Codex files under `plan` names no reset of its own, so the
    /// footer quotes the window that filled up — the same news Claude gets.
    #[test]
    fn a_spent_codex_plan_shows_when_it_comes_back() {
        let mut full = reading(Provider::Codex, "5h", QuotaStatus::Exhausted, Some(1.0));
        full.resets_at_ms = Some(4_102_444_800_000);
        let mut refusal = reading(Provider::Codex, "plan", QuotaStatus::Exhausted, None);
        refusal.at_ms = full.at_ms;
        let segments = footer_segments(&[&full, &refusal], 1_000);
        assert!(
            text(&segments).starts_with(" codex: resets "),
            "{segments:?}"
        );
        assert_eq!(segments[1].tone, Tone::Error);
        // The reset replaces the figures rather than crowding in beside them.
        assert!(!text(&segments).contains('%'), "{segments:?}");
    }

    /// An account out of credits comes back when somebody pays, not when a
    /// clock turns over — and the last percentages would read as room to spare.
    #[test]
    fn a_codex_refusal_naming_no_reset_still_says_the_plan_is_spent() {
        let mut healthy = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.12));
        healthy.at_ms = 1;
        let mut refusal = reading(Provider::Codex, "plan", QuotaStatus::Exhausted, None);
        refusal.at_ms = 2;
        refusal.detail = Some("workspace_member_credits_depleted".into());
        assert_eq!(
            text(&footer_segments(&[&healthy, &refusal], 3)),
            " codex: spent"
        );
    }

    /// Once the provider serves again, or the moment it named has passed, the
    /// footer goes back to the figures rather than quoting a stale reset.
    #[test]
    fn a_codex_plan_that_is_serving_again_shows_its_figures() {
        let mut refusal = reading(Provider::Codex, "plan", QuotaStatus::Exhausted, None);
        refusal.at_ms = 1;
        refusal.resets_at_ms = Some(2_000);
        let mut served = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.04));
        served.at_ms = 3;
        assert_eq!(
            text(&footer_segments(&[&refusal, &served], 1)),
            " codex: 4%"
        );
        // The reset alone is enough, even before any reading contradicts it.
        assert_eq!(text(&footer_segments(&[&refusal], 3_000)), "");
    }

    /// One provider's spent plan says nothing about the other's.
    #[test]
    fn a_spent_codex_plan_leaves_the_claude_reading_alone() {
        let mut refusal = reading(Provider::Codex, "plan", QuotaStatus::Exhausted, None);
        refusal.resets_at_ms = Some(4_102_444_800_000);
        let claude = reading(
            Provider::Claude,
            "five_hour",
            QuotaStatus::Warning,
            Some(0.91),
        );
        let text = text(&footer_segments(&[&refusal, &claude], 1_000));
        assert!(text.starts_with(" codex: resets "), "{text}");
        assert!(text.ends_with(" claude: 91%"), "{text}");
    }

    #[test]
    fn latest_clear_claude_reading_is_a_dash() {
        let mut old = reading(
            Provider::Claude,
            "five_hour",
            QuotaStatus::Warning,
            Some(0.91),
        );
        old.at_ms = 1;
        let mut clear = reading(Provider::Claude, "five_hour", QuotaStatus::Allowed, None);
        clear.at_ms = 2;
        assert_eq!(text(&footer_segments(&[&old, &clear], 0)), " claude: -");
    }
    #[test]
    fn exhausted_claude_reset_uses_supplied_now() {
        let mut exhausted = reading(Provider::Claude, "five_hour", QuotaStatus::Exhausted, None);
        exhausted.resets_at_ms = Some(4_102_444_800_000);
        assert!(text(&footer_segments(&[&exhausted], 1)).contains("resets"));
        assert_eq!(
            text(&footer_segments(&[&exhausted], u64::MAX)),
            " claude: -"
        );
    }
    #[test]
    fn time_format_handles_fractional_and_negative_offsets() {
        let reset = 1_788_290_400_000;
        assert_eq!(minute_of_day(reset, 5 * 3_600 + 1_800), "00:50");
        assert_eq!(minute_of_day(0, -3_600), "23:00");
    }
}
