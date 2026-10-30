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
            worktree: None,
            source_branch: None,
            parent: None,
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
    fn rendered(readings: &[&QuotaEvent], auto_retry: bool, now_ms: u64, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, height)).unwrap();
        terminal
            .draw(|frame| {
                render(
                    frame,
                    &QuotaView {
                        chrome: chrome(),
                        readings,
                        auto_retry,
                        scroll_back: 0,
                        now_ms,
                    },
                    frame.area(),
                )
            })
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
    fn view_renders_reading_and_retry_state() {
        let reading = reading(Provider::Codex, "5h", QuotaStatus::Warning, Some(0.81));
        let output = rendered(&[&reading], true, 1_000, 10);
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
        let output = rendered(&[&old, &weekly], true, 2_000, 10);
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
        let output = rendered(&borrowed, false, 1_000, 40);
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

    #[test]
    fn only_the_newest_refusal_with_a_future_reset_moves_the_footer() {
        let figure = reading(Provider::Codex, "5h", QuotaStatus::Warning, Some(0.91));
        let mut refused = reading(Provider::Codex, "5h", QuotaStatus::Exhausted, Some(1.0));
        refused.resets_at_ms = Some(9_000);
        let mut undated = reading(Provider::Codex, "5h", QuotaStatus::Exhausted, None);
        undated.resets_at_ms = None;
        let mut serving = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.1));
        serving.at_ms = refused.at_ms + 1;
        for (why, readings, now, expected) in [
            (
                "a figure stands until replaced",
                vec![&figure],
                1_000,
                false,
            ),
            ("the moment is ahead", vec![&refused], 1_000, true),
            ("the moment has passed", vec![&refused], 9_001, false),
            ("spent names no moment", vec![&undated], 1_000, false),
            (
                "serving retires the refusal",
                vec![&refused, &serving],
                1_000,
                false,
            ),
        ] {
            assert_eq!(footer_ticks(&readings, now), expected, "{why}");
        }
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
