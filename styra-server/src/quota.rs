//! Plan-quota tracking: what the providers say about how much of the
//! subscription is left, kept where an operator can see it.
//!
//! Both interactive providers volunteer quota figures on the wire, and Genta's
//! decoder drops them: Claude's `rate_limit_event` becomes an
//! [`AgentEvent::Unknown`](crate::event::AgentEvent::Unknown) carrying only its
//! wire type, and Codex's `rate_limits` rides inside a token-count
//! notification whose decoded form keeps the token counts alone. The figures
//! are therefore read here, from the verbatim line, before anything discards
//! them.
//!
//! The log is deliberately in-memory and server-wide. Quota belongs to the
//! account, not to a session, so one interaction's reading is worth showing
//! while another interaction is the one on screen; and it is a live reading
//! rather than a record worth keeping, so it dies with the daemon instead of
//! accumulating in the store next to the journals.
//!
//! Crossing a usage threshold also has to *reach* the operator rather than
//! wait to be looked up, so [`QuotaLog::observe`] hands back the readings the
//! caller should put into the interaction's update stream. It returns a given
//! window only when its reading actually changes — status moving, or usage
//! climbing another ten percent — so a provider that repeats itself every turn
//! costs one announcement, not one per turn. How an announcement is *shown* is
//! the client's business; this decides only that it is worth showing.

use crate::agent::Provider;
use crate::protocol::{QuotaEvent, QuotaStatus};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::Mutex;

/// The usage fraction at or above which a reading is worth telling the
/// operator about unprompted. Claude raises its own `allowed_warning` at the
/// same 0.9, so a Codex window crossing this is treated the same way.
pub const WARN_THRESHOLD: f64 = 0.9;

/// How many readings the log keeps. Readings arrive at most a few times per
/// turn, so this holds a long working session's worth while staying bounded.
const CAPACITY: usize = 512;

/// A server-wide, bounded log of the quota readings seen on any interaction's
/// wire, and the notification state needed to avoid repeating itself.
#[derive(Default)]
pub struct QuotaLog {
    entries: Mutex<VecDeque<QuotaEvent>>,
    /// The last reading reported to the operator per provider window, as its
    /// status and its usage decile. A window whose reading lands on the same
    /// pair is not worth a second message. Keyed by provider as well as name
    /// because the providers name their windows independently, and one
    /// account's Claude limit says nothing about its Codex one.
    reported: Mutex<HashMap<(Provider, String), (QuotaStatus, u8)>>,
}

impl QuotaLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read any quota figures out of one verbatim agent line, record them all,
    /// and return those the operator should be told about now.
    ///
    /// A line carrying no quota figures — nearly all of them — costs one
    /// substring scan and nothing else.
    pub fn observe(
        &self,
        session_id: &str,
        provider: Provider,
        at_ms: u64,
        raw: &str,
    ) -> Vec<QuotaEvent> {
        let readings = parse(session_id, provider, at_ms, raw);
        if readings.is_empty() {
            return Vec::new();
        }
        let mut announce = Vec::new();
        for reading in readings {
            if self.worth_reporting(&reading) {
                announce.push(reading.clone());
            }
            self.record(reading);
        }
        announce
    }

    /// Whether this reading says something the operator has not already been
    /// told: an exhausted or newly-warning window, or one that has climbed
    /// another decile since it was last reported.
    fn worth_reporting(&self, reading: &QuotaEvent) -> bool {
        if !reading.is_notable() {
            return false;
        }
        let decile = reading
            .utilization
            .map(|used| (used * 10.0) as u8)
            .unwrap_or_default();
        let mut reported = self.reported.lock().expect("quota report lock poisoned");
        let key = (reading.provider, reading.window.clone());
        match reported.get(&key) {
            Some((status, seen)) if *status == reading.status && *seen >= decile => false,
            _ => {
                reported.insert(key, (reading.status, decile));
                true
            }
        }
    }

    fn record(&self, reading: QuotaEvent) {
        let mut entries = self.entries.lock().expect("quota log lock poisoned");
        if entries.len() == CAPACITY {
            entries.pop_front();
        }
        entries.push_back(reading);
    }

    /// Every reading held, oldest first.
    pub fn entries(&self) -> Vec<QuotaEvent> {
        self.entries
            .lock()
            .expect("quota log lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

/// Read the quota figures out of one agent line, if it carries any.
///
/// The line is searched by key rather than matched against a known envelope:
/// Claude puts `rate_limit_info` at the top level of a `rate_limit_event`,
/// Codex nests `rate_limits` inside a notification's `params`, and neither
/// shape is worth restating here to find a field both simply name.
fn parse(session_id: &str, provider: Provider, at_ms: u64, raw: &str) -> Vec<QuotaEvent> {
    // Cheap reject first: parsing every tool result and message body as JSON
    // to look for a field almost none of them have is the common case.
    if !raw.contains("rate_limit") {
        return Vec::new();
    }
    let Ok(line) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let mut readings = Vec::new();
    // Claude names the object in the singular, and named it `rate_limit`
    // before `rate_limit_info`; both carry the same fields.
    for found in find_all(&line, "rate_limit_info")
        .into_iter()
        .chain(find_all(&line, "rate_limit"))
    {
        readings.extend(claude_reading(session_id, provider, at_ms, found));
    }
    for found in find_all(&line, "rate_limits") {
        readings.extend(codex_readings(session_id, provider, at_ms, found));
    }
    readings
}

/// Every value under `key`, at any depth.
fn find_all<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    let mut found = Vec::new();
    collect(value, key, &mut found);
    found
}

fn collect<'a>(value: &'a Value, key: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            for (name, nested) in map {
                if name == key {
                    found.push(nested);
                }
                collect(nested, key, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, key, found);
            }
        }
        _ => {}
    }
}

/// Claude's reading: one window, named by `rateLimitType`, whose status the
/// provider decides for itself. `utilization` is only present once the
/// provider has something to warn about, so a plain `allowed` reading records
/// the window and its reset without claiming to know how full it is.
fn claude_reading(
    session_id: &str,
    provider: Provider,
    at_ms: u64,
    info: &Value,
) -> Option<QuotaEvent> {
    let info = info.as_object()?;
    let status = match info.get("status").and_then(Value::as_str)? {
        "allowed" | "ok" => QuotaStatus::Allowed,
        "allowed_warning" => QuotaStatus::Warning,
        "rejected" | "exhausted" => QuotaStatus::Exhausted,
        // An unrecognised status is still a reading; treat it as unremarkable
        // rather than inventing a severity for it.
        _ => QuotaStatus::Allowed,
    };
    Some(QuotaEvent {
        at_ms,
        session_id: session_id.to_owned(),
        provider,
        window: info
            .get("rateLimitType")
            .and_then(Value::as_str)
            .unwrap_or("plan")
            .to_owned(),
        status,
        utilization: info.get("utilization").and_then(Value::as_f64),
        resets_at_ms: info
            .get("resetsAt")
            .and_then(Value::as_u64)
            .map(|seconds| seconds * 1_000),
        detail: info
            .get("overageDisabledReason")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// Codex's reading: a `primary` and `secondary` window, each with its own
/// length and percentage. It reports a number on every turn and no status of
/// its own, so the status here comes from the percentage against
/// [`WARN_THRESHOLD`].
fn codex_readings(
    session_id: &str,
    provider: Provider,
    at_ms: u64,
    limits: &Value,
) -> Vec<QuotaEvent> {
    let Some(limits) = limits.as_object() else {
        return Vec::new();
    };
    let mut readings = Vec::new();
    for name in ["primary", "secondary"] {
        let Some(window) = limits.get(name).and_then(Value::as_object) else {
            continue;
        };
        let Some(used_percent) = window.get("used_percent").and_then(Value::as_f64) else {
            continue;
        };
        let utilization = used_percent / 100.0;
        readings.push(QuotaEvent {
            at_ms,
            session_id: session_id.to_owned(),
            provider,
            window: window
                .get("limit_name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(
                    || match window.get("window_minutes").and_then(Value::as_u64) {
                        Some(minutes) => humanize_window(minutes),
                        None => name.to_owned(),
                    },
                ),
            status: if utilization >= 1.0 {
                QuotaStatus::Exhausted
            } else if utilization >= WARN_THRESHOLD {
                QuotaStatus::Warning
            } else {
                QuotaStatus::Allowed
            },
            utilization: Some(utilization),
            resets_at_ms: window
                .get("resets_at")
                .and_then(Value::as_u64)
                .map(|seconds| seconds * 1_000),
            detail: None,
        });
    }
    readings
}

/// A window length as an operator would say it: the figures come in minutes,
/// and "7d" reads where "10080 minutes" does not.
fn humanize_window(minutes: u64) -> String {
    if minutes.is_multiple_of(1_440) {
        format!("{}d", minutes / 1_440)
    } else if minutes.is_multiple_of(60) {
        format!("{}h", minutes / 60)
    } else {
        format!("{minutes}m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape Claude actually sends, taken from a stored journal.
    const CLAUDE_WARNING: &str = r#"{"type":"rate_limit_event","rate_limit_info":
        {"status":"allowed_warning","resetsAt":1788290400,"rateLimitType":"five_hour",
         "utilization":0.91,"isUsingOverage":false,"surpassedThreshold":0.9}}"#;
    const CLAUDE_ALLOWED: &str = r#"{"type":"rate_limit_event","rate_limit_info":
        {"status":"allowed","resetsAt":1788290400,"rateLimitType":"five_hour",
         "overageStatus":"rejected","overageDisabledReason":"out_of_credits",
         "isUsingOverage":false}}"#;
    const CLAUDE_REJECTED: &str = r#"{"type":"rate_limit_event","rate_limit_info":
        {"status":"rejected","resetsAt":1788290400,"rateLimitType":"five_hour",
         "overageStatus":"rejected","overageDisabledReason":"out_of_credits",
         "isUsingOverage":false}}"#;
    /// Codex nests its figures inside a token-count notification.
    const CODEX_USAGE: &str = r#"{"method":"item/updated","params":{"update":
        {"type":"token_count","rate_limits":{
            "primary":{"used_percent":0.0,"window_minutes":60,"resets_at":1788290400},
            "secondary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1788390400},
            "credits":null}}}}"#;

    #[test]
    fn a_claude_warning_is_read_with_its_window_and_usage() {
        let readings = parse("s-1", Provider::Claude, 1_000, CLAUDE_WARNING);
        assert_eq!(readings.len(), 1);
        let reading = &readings[0];
        assert_eq!(reading.session_id, "s-1");
        assert_eq!(reading.at_ms, 1_000);
        assert_eq!(reading.window, "five_hour");
        assert_eq!(reading.status, QuotaStatus::Warning);
        assert_eq!(reading.utilization, Some(0.91));
        assert_eq!(reading.resets_at_ms, Some(1_788_290_400_000));
        assert!(reading.is_notable());
    }

    /// An `allowed` reading carries no utilization at all, which is a fact
    /// about the provider worth keeping rather than a zero worth inventing.
    #[test]
    fn a_permitted_claude_reading_has_no_usage_figure_and_is_not_notable() {
        let readings = parse("s-1", Provider::Claude, 1_000, CLAUDE_ALLOWED);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].status, QuotaStatus::Allowed);
        assert_eq!(readings[0].utilization, None);
        assert_eq!(readings[0].detail.as_deref(), Some("out_of_credits"));
        assert!(!readings[0].is_notable());
    }

    #[test]
    fn a_rejection_is_notable_even_without_a_usage_figure() {
        let readings = parse("s-1", Provider::Claude, 1_000, CLAUDE_REJECTED);
        assert_eq!(readings[0].status, QuotaStatus::Exhausted);
        assert!(readings[0].is_notable());
    }

    /// Codex reports both its windows in one line, as percentages rather than
    /// fractions, and says nothing about severity.
    #[test]
    fn both_codex_windows_are_read_and_their_percentages_normalized() {
        let readings = parse("s-2", Provider::Codex, 2_000, CODEX_USAGE);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].window, "1h");
        assert_eq!(readings[0].utilization, Some(0.0));
        assert_eq!(readings[0].status, QuotaStatus::Allowed);
        assert_eq!(readings[1].window, "7d");
        assert_eq!(readings[1].utilization, Some(0.125));
        assert_eq!(readings[1].resets_at_ms, Some(1_788_390_400_000));
        assert!(!readings[1].is_notable());
    }

    /// Codex states no threshold of its own, so a window past
    /// [`WARN_THRESHOLD`] has to be recognised here or it passes unnoticed.
    #[test]
    fn a_codex_window_past_the_threshold_warns_and_a_full_one_is_exhausted() {
        let line = |percent: f64| {
            format!(
                r#"{{"params":{{"rate_limits":{{"primary":{{"used_percent":{percent},"window_minutes":60}}}}}}}}"#
            )
        };
        assert_eq!(
            parse("s-1", Provider::Claude, 0, &line(93.0))[0].status,
            QuotaStatus::Warning
        );
        assert_eq!(
            parse("s-1", Provider::Claude, 0, &line(100.0))[0].status,
            QuotaStatus::Exhausted
        );
        assert_eq!(
            parse("s-1", Provider::Claude, 0, &line(40.0))[0].status,
            QuotaStatus::Allowed
        );
    }

    #[test]
    fn lines_carrying_no_quota_figures_are_ignored() {
        assert!(parse(
            "s-1",
            Provider::Claude,
            0,
            r#"{"type":"assistant","message":{}}"#
        )
        .is_empty());
        assert!(parse("s-1", Provider::Claude, 0, "not json at all").is_empty());
        // A message that merely mentions the field name must not be mistaken
        // for a reading.
        assert!(parse(
            "s-1",
            Provider::Claude,
            0,
            r#"{"text":"grep rate_limit journal.jsonl"}"#
        )
        .is_empty());
    }

    #[test]
    fn readings_are_logged_in_arrival_order_and_bounded() {
        let log = QuotaLog::new();
        for at_ms in 0..(CAPACITY as u64 + 10) {
            log.observe("s-1", Provider::Codex, at_ms, CODEX_USAGE);
        }
        let entries = log.entries();
        assert_eq!(entries.len(), CAPACITY);
        // The oldest readings were dropped, and what is left is still ordered.
        assert!(entries
            .windows(2)
            .all(|pair| pair[0].at_ms <= pair[1].at_ms));
        assert_eq!(entries[entries.len() - 1].at_ms, CAPACITY as u64 + 9);
    }

    /// The point of the notification state: a provider that repeats the same
    /// warning on every turn must not repeat the message on every turn.
    #[test]
    fn a_repeated_warning_is_announced_once_but_still_logged_each_time() {
        let log = QuotaLog::new();
        let first = log.observe("s-1", Provider::Claude, 1, CLAUDE_WARNING);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].status, QuotaStatus::Warning);
        assert!(first[0].describe().contains("91%"));
        assert!(log
            .observe("s-1", Provider::Claude, 2, CLAUDE_WARNING)
            .is_empty());
        assert!(log
            .observe("s-1", Provider::Claude, 3, CLAUDE_WARNING)
            .is_empty());
        assert_eq!(log.entries().len(), 3);
    }

    /// Usage climbing within a warning is worth saying again — but only once
    /// it has moved a decile, not on every percent.
    #[test]
    fn a_warning_is_repeated_when_usage_climbs_a_decile() {
        let reading = |utilization: f64| {
            format!(
                r#"{{"rate_limit_info":{{"status":"allowed_warning","rateLimitType":"five_hour",
                   "utilization":{utilization}}}}}"#
            )
        };
        let log = QuotaLog::new();
        assert_eq!(
            log.observe("s-1", Provider::Claude, 1, &reading(0.91))
                .len(),
            1
        );
        // Still the same decile: already said.
        assert!(log
            .observe("s-1", Provider::Claude, 2, &reading(0.95))
            .is_empty());
        assert!(log
            .observe("s-1", Provider::Claude, 3, &reading(0.99))
            .is_empty());
        // A full window is a different status, so it is announced again.
        let exhausted = log.observe("s-1", Provider::Claude, 4, CLAUDE_REJECTED);
        assert_eq!(exhausted.len(), 1);
        assert_eq!(exhausted[0].status, QuotaStatus::Exhausted);
    }

    /// Windows are tracked separately: a five-hour limit filling up says
    /// nothing about the weekly one, and each gets its own message.
    #[test]
    fn each_window_is_announced_on_its_own() {
        let log = QuotaLog::new();
        let line = r#"{"params":{"rate_limits":{
            "primary":{"used_percent":95.0,"window_minutes":60},
            "secondary":{"used_percent":92.0,"window_minutes":10080}}}}"#;
        let announced = log.observe("s-1", Provider::Codex, 1, line);
        assert_eq!(announced.len(), 2);
        assert_eq!(announced[0].window, "1h");
        assert_eq!(announced[1].window, "7d");
    }

    /// The reading has to say whose plan it is: the two providers are
    /// separate subscriptions, and the log mixes both accounts' readings.
    #[test]
    fn a_reading_names_the_provider_whose_plan_it_measures() {
        let claude = parse("s-1", Provider::Claude, 1_000, CLAUDE_WARNING);
        assert_eq!(claude[0].provider, Provider::Claude);
        assert!(claude[0].describe().starts_with("claude plan quota"));
        let codex = parse("s-2", Provider::Codex, 2_000, CODEX_USAGE);
        assert!(codex.iter().all(|r| r.provider == Provider::Codex));
    }

    /// Two providers can name a window alike — and even when they do not, one
    /// account filling up says nothing about the other, so each is announced
    /// on its own.
    #[test]
    fn the_same_window_name_on_two_providers_is_announced_once_each() {
        let log = QuotaLog::new();
        let line = r#"{"rate_limit_info":{"status":"allowed_warning",
            "rateLimitType":"5h","utilization":0.91}}"#;
        assert_eq!(log.observe("s-1", Provider::Claude, 1, line).len(), 1);
        let codex = log.observe("s-2", Provider::Codex, 2, line);
        assert_eq!(codex.len(), 1);
        assert_eq!(codex[0].provider, Provider::Codex);
        // Each provider has now been reported, so neither repeats.
        assert!(log.observe("s-1", Provider::Claude, 3, line).is_empty());
        assert!(log.observe("s-2", Provider::Codex, 4, line).is_empty());
    }

    #[test]
    fn window_lengths_read_as_an_operator_would_say_them() {
        assert_eq!(humanize_window(60), "1h");
        assert_eq!(humanize_window(300), "5h");
        assert_eq!(humanize_window(1_440), "1d");
        assert_eq!(humanize_window(10_080), "7d");
        assert_eq!(humanize_window(90), "90m");
    }
}
