//! Provider quota presentation and compact footer readings.

use crate::{
    chrome::PanelChrome,
    footer::{Segment, Tone},
};
use styra_protocol::{agent::Provider, QuotaEvent, QuotaStatus};

const ERROR: f64 = 0.90;
const WARNING: f64 = 0.75;
pub struct QuotaView<'a> {
    pub chrome: PanelChrome,
    pub readings: &'a [&'a QuotaEvent],
    pub auto_retry: bool,
    pub scroll_back: u16,
    /// Time is supplied by the adapter so fixtures and mocks stay deterministic.
    pub now_ms: u64,
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

/// Whether the footer these readings produce will say something different
/// later on with no new reading to make it so.
///
/// Almost none of it will. The percentages are figures taken at a moment and
/// stand until the next reading replaces them, and a reset is quoted as the
/// wall-clock moment it falls at rather than counted down to — so the text is
/// the same text a minute later. The exception is the moment itself passing:
/// both the Codex and the Claude halves stop quoting a reset once it is in the
/// past, and the reading that named it does not change when it does.
///
/// So this is true only while a refusal names a moment still ahead of us, and
/// goes false once that moment lands. The caller is the event loop, which
/// repaints on it; see `presentation::ticking`.
pub fn footer_ticks(readings: &[&QuotaEvent], now_ms: u64) -> bool {
    newest_per_window(readings).iter().any(|reading| {
        reading.status == QuotaStatus::Exhausted
            && reading.resets_at_ms.is_some_and(|reset| reset > now_ms)
    })
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

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/quota.rs"]
mod rendering;
