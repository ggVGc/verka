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

pub struct QuotaView<'a> {
    pub chrome: PanelChrome,
    pub readings: &'a [&'a QuotaEvent],
    pub auto_retry: bool,
    pub scroll_back: u16,
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
    let lines = view
        .readings
        .iter()
        .map(|reading| line(reading))
        .collect::<Vec<_>>();
    let start = lines
        .len()
        .saturating_sub(area.height.saturating_sub(2) as usize)
        .saturating_sub(view.scroll_back as usize) as u16;
    frame.render_widget(Paragraph::new(lines).block(block).scroll((start, 0)), area);
}

fn line(reading: &QuotaEvent) -> Line<'static> {
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
            format!("resets {} ", clock(reset)),
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
    let mut newest: Vec<&QuotaEvent> = Vec::new();
    for reading in readings {
        match newest
            .iter_mut()
            .find(|kept| kept.provider == reading.provider && kept.window == reading.window)
        {
            Some(kept) if kept.at_ms <= reading.at_ms => *kept = reading,
            Some(_) => {}
            None => newest.push(reading),
        }
    }
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
    if codex.is_empty() && claude.is_none() {
        return Vec::new();
    }
    let mut output = Vec::new();
    if !codex.is_empty() {
        output.push(segment(" codex: ", Tone::Muted, false));
    }
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
    fn footer_keeps_concrete_codex_windows_and_tones() {
        let primary = reading(Provider::Codex, "5h", QuotaStatus::Allowed, Some(0.12));
        let weekly = reading(Provider::Codex, "7d", QuotaStatus::Warning, Some(0.91));
        let segments = footer_segments(&[&primary, &weekly], 0);
        assert_eq!(text(&segments), " codex: 12%/91%");
        assert_eq!(segments[2].tone, Tone::Error);
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
