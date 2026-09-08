//! Plan-quota tracking: what the providers say about how much of the
//! subscription is left, kept where an operator can see it.
//!
//! Both interactive providers volunteer quota figures on the wire, and Genta's
//! decoder drops them: Claude's `rate_limit_event` becomes an
//! [`AgentEvent::Unknown`](crate::event::AgentEvent::Unknown) carrying only its
//! wire type, and Codex's limits arrive on a notification whose decoded form
//! keeps nothing of them. The figures are therefore read here, from the
//! verbatim line, before anything discards them.
//!
//! Reading the verbatim line means tracking what the providers call things.
//! Codex has moved its figures out of the token-count notification they used to
//! ride in, into an `account/rateLimits/updated` of their own, and renamed every
//! field to camelCase (`rateLimits`, `usedPercent`, `windowDurationMins`,
//! `resetsAt`) — a rename that read as silence rather than as an error, and cost
//! every Codex reading until it was noticed. Both spellings are therefore
//! searched, and asked for by name rather than by envelope: see
//! [`CODEX_LIMITS_KEYS`] and [`field`].
//!
//! The figures are not the only thing to read, because the moment they matter
//! most is the moment Codex stops sending them: it reports its percentages on
//! turns that run, and a turn refused for a spent plan carries none at all —
//! only an `error` notification and a failed `turn/completed`, both naming
//! `usageLimitExceeded`. Those refusals are read here too (see
//! [`codex_refusals`]), along with the `rateLimitReachedType` Codex sets on the
//! limits object itself when a plan is spent rather than merely filling, so
//! that hitting the limit is the one thing this log cannot miss.
//!
//! The log is server-wide rather than per-session, because quota belongs to
//! the account: one interaction's reading is worth showing while another
//! interaction is the one on screen. It is kept in the store as well as in
//! memory, so that a restarted daemon — or the first session of the morning —
//! opens already knowing where each window stood, rather than showing an empty
//! view until some provider happens to volunteer a figure again.
//!
//! Durable does not mean unbounded. Readings arrive several times a turn and
//! are worth nothing once their window has turned over, so the log keeps only
//! [`RETENTION_MS`] of history and at most [`CAPACITY`] readings, always
//! sparing the newest reading of each provider window from the count so that
//! "where does each window stand" stays answerable however chatty the
//! session was. See [`trim`].
//!
//! Crossing a usage threshold also has to *reach* the operator rather than
//! wait to be looked up, so [`QuotaLog::observe`] hands back the readings the
//! caller should put into the interaction's update stream. It returns a given
//! window only when its reading actually changes — status moving, or usage
//! climbing another ten percent — so a provider that repeats itself every turn
//! costs one announcement, not one per turn. Said once is not said forever,
//! though: a window that reads empty again has turned over, and the next time
//! it fills is news, so evidence of room forgets what was said about it. How an
//! announcement is *shown* is the client's business; this decides only that it
//! is worth showing.

use crate::agent::Provider;
use crate::protocol::{QuotaEvent, QuotaStatus};
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The usage fraction at or above which a reading is worth telling the
/// operator about unprompted. Claude raises its own `allowed_warning` at the
/// same 0.9, so a Codex window crossing this is treated the same way.
pub const WARN_THRESHOLD: f64 = 0.9;

/// How empty a window has to read before an earlier warning about it counts as
/// spent. Usage climbs towards a limit and falls to nothing when the window
/// turns over, so a window under half of [`WARN_THRESHOLD`] is a fresh window
/// whose next filling is worth announcing again — whereas one sitting at 88%
/// is the same nearly-full window that was already announced, and re-arming on
/// that would let a figure wobbling either side of the threshold repeat the
/// same warning all afternoon.
const TURNOVER_THRESHOLD: f64 = WARN_THRESHOLD / 2.0;

/// The window a reading is filed under when the provider names none. Claude
/// omits its `rateLimitType` on occasion, and a Codex refusal states no window
/// whatsoever — both mean the account's plan as a whole.
const PLAN_WINDOW: &str = "plan";

/// The `codexErrorInfo` on a turn Codex refused because the plan is spent.
/// Codex sends its percentages only on turns that run, so on such a turn this
/// string is the whole of what the wire says about the quota.
const USAGE_LIMIT_EXCEEDED: &str = "usageLimitExceeded";

/// What Codex has called the object holding its two windows, newest first: it
/// renamed the whole payload to camelCase and moved it into a notification of
/// its own. Both are searched rather than the newer one alone, so that a
/// journal recorded by either build reads the same — and so that the next
/// rename costs one entry here.
const CODEX_LIMITS_KEYS: [&str; 2] = ["rateLimits", "rate_limits"];

/// How many readings the log keeps. Readings arrive at most a few times per
/// turn, so this holds a long working session's worth while staying bounded.
const CAPACITY: usize = 512;

/// How much history is worth keeping. The longest window either provider
/// reports is a week, so a reading older than this describes a window that has
/// since turned over twice and says nothing about what is left now.
const RETENTION_MS: u64 = 14 * 24 * 60 * 60 * 1_000;

/// The store file holding the readings, beside the workspaces rather than
/// inside one: the account's quota is not any workspace's business.
const QUOTA_FILE: &str = "quota.jsonl";

/// A server-wide, bounded, store-backed log of the quota readings seen on any
/// interaction's wire, and the notification state needed to avoid repeating
/// itself.
#[derive(Default)]
pub struct QuotaLog {
    /// Where the readings are kept between runs. `None` for a log that exists
    /// only for the life of the process, which is what the tests and a server
    /// with no store want.
    path: Option<PathBuf>,
    entries: Mutex<VecDeque<QuotaEvent>>,
    /// The last reading reported to the operator per provider window, as its
    /// status and its usage decile. A window whose reading lands on the same
    /// pair is not worth a second message. Keyed by provider as well as name
    /// because the providers name their windows independently, and one
    /// account's Claude limit says nothing about its Codex one.
    reported: Mutex<HashMap<(Provider, String), (QuotaStatus, u8)>>,
}

impl QuotaLog {
    /// A log that keeps nothing past the life of the process.
    pub fn new() -> Self {
        Self::default()
    }

    /// The store's log, with whatever readings a previous run left behind.
    ///
    /// A missing file is the ordinary first run, and an unreadable or
    /// half-written one is not worth refusing to start over: a quota reading
    /// is a live figure that the providers will volunteer again within a turn,
    /// so a damaged log is reported and then replaced by what this run sees.
    pub fn open(store_root: &Path) -> Self {
        let path = store_root.join(QUOTA_FILE);
        let mut entries = match read(&path) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("styra-server: reopening the quota log: {error:#}");
                VecDeque::new()
            }
        };
        // History kept across a restart is exactly the history most likely to
        // have gone stale, so it is trimmed on the way in rather than only on
        // the way out.
        trim(&mut entries);
        Self {
            path: Some(path),
            entries: Mutex::new(entries),
            // Deliberately not restored: the announcements a previous run made
            // went to interactions that no longer exist, so a window still
            // sitting at 95% is worth saying once more to this run's operator.
            reported: Mutex::default(),
        }
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
    ///
    /// A reading showing room says nothing worth announcing, but it does clear
    /// what was announced about that window, so that the window's next filling
    /// is told about rather than mistaken for the one already reported. It
    /// clears the provider's [`PLAN_WINDOW`] with it: a refusal is filed there
    /// for want of a window name, and a provider now reporting room is a
    /// provider running turns again, which is exactly what a refusal said it
    /// would not do.
    fn worth_reporting(&self, reading: &QuotaEvent) -> bool {
        let mut reported = self.reported.lock().expect("quota report lock poisoned");
        if !reading.is_notable() {
            if shows_room(reading) {
                reported.remove(&(reading.provider, reading.window.clone()));
                reported.remove(&(reading.provider, PLAN_WINDOW.to_owned()));
            }
            return false;
        }
        let decile = reading
            .utilization
            .map(|used| (used * 10.0) as u8)
            .unwrap_or_default();
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
        entries.push_back(reading);
        trim(&mut entries);
        let Some(path) = &self.path else {
            return;
        };
        // Written under the lock, so that concurrent readings on two
        // interactions cannot publish the file in the other's order and leave
        // it disagreeing with memory. The file is bounded by `CAPACITY`, so
        // rewriting it whole is cheaper than the parsing that produced the
        // reading.
        if let Err(error) = write(path, &entries) {
            eprintln!("styra-server: keeping the quota log: {error:#}");
        }
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

/// Whether an unremarkable reading is evidence that its window has turned over
/// since it was last announced, rather than merely sitting a shade under the
/// threshold. See [`TURNOVER_THRESHOLD`].
fn shows_room(reading: &QuotaEvent) -> bool {
    match reading.utilization {
        Some(used) => used < TURNOVER_THRESHOLD,
        // Claude sends a figure only once it has something to warn about, so a
        // permitted reading without one is the provider saying there is room.
        None => true,
    }
}

/// Drop the readings no longer worth keeping, oldest first.
///
/// Two rules, in order. Anything older than [`RETENTION_MS`] goes: a window
/// that has since reset says nothing about what is left now. Age is measured
/// against the newest reading held rather than the wall clock, so a log that
/// stops being written keeps what it had instead of quietly emptying itself,
/// and so the rule is the same in a test as on a machine whose clock moved.
///
/// What remains is then cut to [`CAPACITY`], except that the newest reading of
/// each provider window is never the one dropped. Without that, a busy session
/// reporting one window every turn would push every other window's last known
/// figure out of a log whose whole purpose is to say where each of them
/// stands.
fn trim(entries: &mut VecDeque<QuotaEvent>) {
    let Some(newest) = entries.back().map(|reading| reading.at_ms) else {
        return;
    };
    let horizon = newest.saturating_sub(RETENTION_MS);
    while entries
        .front()
        .is_some_and(|reading| reading.at_ms < horizon)
    {
        entries.pop_front();
    }
    let Some(mut excess) = entries.len().checked_sub(CAPACITY).filter(|over| *over > 0) else {
        return;
    };
    // Built by position, oldest first, so the later reading of a window
    // overwrites the earlier one and what is left is each window's newest.
    let newest_of_window: HashMap<(Provider, &str), usize> = entries
        .iter()
        .enumerate()
        .map(|(index, reading)| ((reading.provider, reading.window.as_str()), index))
        .collect();
    let spared: std::collections::HashSet<usize> = newest_of_window.into_values().collect();
    let mut index = 0;
    entries.retain(|_| {
        let position = index;
        index += 1;
        if excess == 0 || spared.contains(&position) {
            return true;
        }
        excess -= 1;
        false
    });
}

/// The readings a previous run left in the store, oldest first.
///
/// One JSON object per line, as the journals are written: an entry the current
/// build cannot parse is skipped rather than failing the whole file, so a
/// reading whose shape has since changed costs its own line and no more.
fn read(path: &Path) -> Result<VecDeque<QuotaEvent>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // No file is the ordinary first run, not a problem to report.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(VecDeque::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Publish the whole log, atomically: a reader (this daemon's next run) must
/// never see a file half-written by a reading that arrived mid-rewrite.
fn write(path: &Path, entries: &VecDeque<QuotaEvent>) -> Result<()> {
    let mut text = String::new();
    for reading in entries {
        text.push_str(&serde_json::to_string(reading).context("serializing a quota reading")?);
        text.push('\n');
    }
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".{QUOTA_FILE}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        std::fs::remove_file(&temporary).ok();
        return Err(error).with_context(|| format!("publishing {}", path.display()));
    }
    Ok(())
}

/// Read the quota figures out of one agent line, if it carries any.
///
/// The line is searched by key rather than matched against a known envelope:
/// Claude puts `rate_limit_info` at the top level of a `rate_limit_event`,
/// Codex nests its limits inside a notification's `params` and its refusals
/// two levels inside an `error` or a `turn/completed`, and none of those
/// envelopes is worth restating here to find a field each simply names. It is
/// also the shape most likely to be renamed under us — as Codex's already was
/// — and a search by key survives a payload moving house.
fn parse(session_id: &str, provider: Provider, at_ms: u64, raw: &str) -> Vec<QuotaEvent> {
    // Cheap reject first: parsing every tool result and message body as JSON
    // to look for fields almost none of them have is the common case. Both
    // spellings have to appear here, or the scan rejects exactly the lines the
    // parsing below exists for.
    if !raw.contains("rate_limit")
        && !raw.contains("rateLimit")
        && !raw.contains(USAGE_LIMIT_EXCEEDED)
    {
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
    for key in CODEX_LIMITS_KEYS {
        for found in find_all(&line, key) {
            readings.extend(codex_readings(session_id, provider, at_ms, found));
        }
    }
    readings.extend(codex_refusals(session_id, provider, at_ms, &line));
    readings
}

/// The first of `names` the object actually has. Codex renamed every field of
/// its limits payload to camelCase, and a reading is worth the same under
/// either spelling, so each field is asked for by both.
fn field<'a>(map: &'a serde_json::Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    names.iter().find_map(|name| map.get(*name))
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

/// Every object, at any depth, whose `key` is the string `expected`. The
/// enclosing object is what is wanted rather than the matched value, because
/// what else the object says — the refusal's own message — sits beside the code
/// rather than under it.
fn find_objects_saying<'a>(value: &'a Value, key: &str, expected: &str) -> Vec<&'a Value> {
    let mut found = Vec::new();
    collect_saying(value, key, expected, &mut found);
    found
}

fn collect_saying<'a>(value: &'a Value, key: &str, expected: &str, found: &mut Vec<&'a Value>) {
    match value {
        Value::Object(map) => {
            if map.get(key).and_then(Value::as_str) == Some(expected) {
                found.push(value);
            }
            for nested in map.values() {
                collect_saying(nested, key, expected, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_saying(item, key, expected, found);
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
            .unwrap_or(PLAN_WINDOW)
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
///
/// Either window can be `null` — a plan with no weekly limit, or an account
/// whose credits are gone and whose windows have stopped meaning anything —
/// and a window that says nothing is skipped rather than recorded as a zero.
/// What such a payload does carry is `rateLimitReachedType`, which is Codex
/// saying the plan is spent rather than filling, and that is a reading of its
/// own.
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
        let Some(used_percent) =
            field(window, &["usedPercent", "used_percent"]).and_then(Value::as_f64)
        else {
            continue;
        };
        let utilization = used_percent / 100.0;
        readings.push(QuotaEvent {
            at_ms,
            session_id: session_id.to_owned(),
            provider,
            window: field(window, &["limitName", "limit_name"])
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    match field(window, &["windowDurationMins", "window_minutes"])
                        .and_then(Value::as_u64)
                    {
                        Some(minutes) => humanize_window(minutes),
                        None => name.to_owned(),
                    }
                }),
            status: if utilization >= 1.0 {
                QuotaStatus::Exhausted
            } else if utilization >= WARN_THRESHOLD {
                QuotaStatus::Warning
            } else {
                QuotaStatus::Allowed
            },
            utilization: Some(utilization),
            resets_at_ms: field(window, &["resetsAt", "resets_at"])
                .and_then(Value::as_u64)
                .map(|seconds| seconds * 1_000),
            detail: None,
        });
    }
    if let Some(reached) = limits
        .get("rateLimitReachedType")
        .and_then(Value::as_str)
        .filter(|reached| !reached.is_empty())
    {
        readings.push(QuotaEvent {
            at_ms,
            session_id: session_id.to_owned(),
            provider,
            // Codex does not say *which* window it reached, and on the payloads
            // that carry this both windows are commonly null, so this is filed
            // against the plan itself.
            window: PLAN_WINDOW.to_owned(),
            status: QuotaStatus::Exhausted,
            utilization: None,
            resets_at_ms: None,
            // The reason as Codex names it: an account out of credits
            // (`workspace_member_credits_depleted`) waits on somebody topping
            // it up, where a filled window waits on the clock, and an operator
            // reading the view needs to know which.
            detail: Some(reached.to_owned()),
        });
    }
    readings
}

/// Codex's refusal: an `error` notification, and the failed `turn/completed`
/// behind it, both carrying [`USAGE_LIMIT_EXCEEDED`]. This is not a figure but
/// the fact the figures exist to predict, and it arrives on the one turn that
/// reports no figures at all, so it is read as a reading in its own right — an
/// exhausted window, filed under [`PLAN_WINDOW`] because the refusal names
/// none.
///
/// It states no percentage either, and none is invented: a spent plan is a
/// different fact from a provider saying 100%, and the view prints `?` where a
/// provider withheld a figure. What the prose does carry is when the plan is
/// usable again, kept verbatim by [`retry_advice`].
///
/// Both lines are read rather than one of them, because neither is promised:
/// the log then holds two readings a millisecond apart, which is what the wire
/// said, and the announcement state sees to it that the operator is told once.
fn codex_refusals(
    session_id: &str,
    provider: Provider,
    at_ms: u64,
    line: &Value,
) -> Vec<QuotaEvent> {
    find_objects_saying(line, "codexErrorInfo", USAGE_LIMIT_EXCEEDED)
        .into_iter()
        .map(|error| QuotaEvent {
            at_ms,
            session_id: session_id.to_owned(),
            provider,
            window: PLAN_WINDOW.to_owned(),
            status: QuotaStatus::Exhausted,
            utilization: None,
            resets_at_ms: None,
            detail: error
                .get("message")
                .and_then(Value::as_str)
                .and_then(retry_advice),
        })
        .collect()
}

/// The part of a refusal's prose an operator can act on: when to come back, or
/// what has to happen before they can. The rest of the paragraph sells an
/// upgrade and links a billing page, which on a one-line reading would push the
/// provider, the window and the session off the row.
///
/// A retry moment is left as the provider wrote it rather than parsed into
/// [`QuotaEvent::resets_at_ms`]: Codex states it as local prose — "Sep 9th,
/// 2026 2:01 AM" — naming no zone, and a reset shown as a confidently wrong
/// minute is worse than one quoted as the provider said it.
fn retry_advice(message: &str) -> Option<String> {
    let advice = match message.find("try again at") {
        Some(at) => message.get(at..)?,
        // The other refusal Codex sends is a workspace out of credits, whose
        // message names no moment because there is none to name: it ends when
        // somebody pays. Its first sentence is the whole of the news.
        None => message.split_terminator('.').next()?,
    };
    let advice = advice.trim().trim_end_matches('.');
    (!advice.is_empty()).then(|| advice.to_owned())
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

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "styra-quota-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// One reading, at `at_ms`, of a window that is `used` full.
    fn reading(provider: Provider, window: &str, at_ms: u64, used: f64) -> QuotaEvent {
        QuotaEvent {
            at_ms,
            session_id: "s-1".into(),
            provider,
            window: window.into(),
            status: QuotaStatus::Allowed,
            utilization: Some(used),
            resets_at_ms: None,
            detail: None,
        }
    }

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
    /// Codex once nested its figures inside a token-count notification, in
    /// snake_case. Journals recorded by that build are still read.
    const CODEX_USAGE: &str = r#"{"method":"item/updated","params":{"update":
        {"type":"token_count","rate_limits":{
            "primary":{"used_percent":0.0,"window_minutes":60,"resets_at":1788290400},
            "secondary":{"used_percent":12.5,"window_minutes":10080,"resets_at":1788390400},
            "credits":null}}}}"#;
    /// What Codex sends now, verbatim from a stored journal: a notification of
    /// its own, camelCase throughout, percentages as whole numbers.
    const CODEX_LIMITS: &str = r#"{"method":"account/rateLimits/updated","params":
        {"rateLimits":{"limitId":"codex","limitName":null,
            "primary":{"usedPercent":36,"windowDurationMins":300,"resetsAt":1788912064},
            "secondary":{"usedPercent":26,"windowDurationMins":10080,"resetsAt":1789459328},
            "credits":{"hasCredits":false,"unlimited":false,"balance":"0"},
            "individualLimit":null,"spendControlReached":null,"planType":"plus",
            "rateLimitReachedType":null}},"emittedAtMs":1788900447474}"#;
    /// The same notification once the account is spent: no windows at all, and
    /// the whole of the news in `rateLimitReachedType`.
    const CODEX_CREDITS_DEPLETED: &str = r#"{"method":"account/rateLimits/updated","params":
        {"rateLimits":{"limitId":"premium","limitName":null,"primary":null,"secondary":null,
            "credits":{"hasCredits":false,"unlimited":false,"balance":null},
            "individualLimit":null,"spendControlReached":null,"planType":"team",
            "rateLimitReachedType":"workspace_member_credits_depleted"}},
        "emittedAtMs":1787684271948}"#;
    /// The refusal itself, as the failed turn reports it.
    const CODEX_REFUSAL: &str = r#"{"method":"turn/completed","params":{"threadId":
        "01a082c6-7baf-7c01-978f-9e3a055d6a2b","turn":{"id":"01a082f5-0fba-7d53-90d4-474774aeade9",
        "items":[],"itemsView":"notLoaded","status":"failed","error":{"message":
        "You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro), visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again at Sep 9th, 2026 2:01 AM.",
        "codexErrorInfo":"usageLimitExceeded","additionalDetails":null,"misalignment":null},
        "startedAt":1788903493,"completedAt":1788903623,"durationMs":129931}}}"#;
    /// The `error` notification Codex emits just before that turn fails.
    const CODEX_REFUSAL_ERROR: &str = r#"{"method":"error","params":{"error":{"message":
        "Your workspace is out of credits. Ask your workspace owner to refill in order to continue.",
        "codexErrorInfo":"usageLimitExceeded","additionalDetails":null,"misalignment":null},
        "willRetry":false,"threadId":"01a03a25-4c54-70b3-96ba-ae1339fd2e3e",
        "turnId":"01a03a42-1b2f-76f2-957c-aef4edbfbb2d"},"emittedAtMs":1787684271948}"#;

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

    /// The shape Codex actually sends today. It was renamed out from under this
    /// module — key, fields and envelope all — and until it was read again no
    /// Codex reading reached the log at all, however chatty the session.
    #[test]
    fn the_notification_codex_sends_now_is_read_with_both_its_windows() {
        let readings = parse("s-2", Provider::Codex, 2_000, CODEX_LIMITS);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].window, "5h");
        assert_eq!(readings[0].utilization, Some(0.36));
        assert_eq!(readings[0].status, QuotaStatus::Allowed);
        assert_eq!(readings[0].resets_at_ms, Some(1_788_912_064_000));
        assert_eq!(readings[1].window, "7d");
        assert_eq!(readings[1].utilization, Some(0.26));
        assert_eq!(readings[1].resets_at_ms, Some(1_789_459_328_000));
    }

    /// The cheap reject decides what is even parsed, so a renamed key has to
    /// pass it: `rateLimits` does not contain `rate_limit`.
    #[test]
    fn the_cheap_reject_admits_both_spellings() {
        assert!(!parse("s-1", Provider::Codex, 0, CODEX_LIMITS).is_empty());
        assert!(!parse("s-1", Provider::Claude, 0, CLAUDE_WARNING).is_empty());
    }

    /// An account whose credits are gone reports no windows whatsoever — the
    /// figures are null and only `rateLimitReachedType` says anything.
    #[test]
    fn a_spent_account_reporting_no_windows_is_still_read_as_exhausted() {
        let readings = parse("s-2", Provider::Codex, 2_000, CODEX_CREDITS_DEPLETED);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].window, "plan");
        assert_eq!(readings[0].status, QuotaStatus::Exhausted);
        // A null window is not a window at 0%.
        assert_eq!(readings[0].utilization, None);
        assert_eq!(
            readings[0].detail.as_deref(),
            Some("workspace_member_credits_depleted")
        );
        assert!(readings[0].is_notable());
    }

    /// The reading this module exists for, and the one Codex reports no figure
    /// on: the turn it refuses because the plan is spent.
    #[test]
    fn a_codex_refusal_is_read_as_an_exhausted_plan_with_its_retry_time() {
        let readings = parse("s-2", Provider::Codex, 2_000, CODEX_REFUSAL);
        assert_eq!(readings.len(), 1);
        let reading = &readings[0];
        assert_eq!(reading.provider, Provider::Codex);
        assert_eq!(reading.window, "plan");
        assert_eq!(reading.status, QuotaStatus::Exhausted);
        // No figure is invented from a refusal that states none.
        assert_eq!(reading.utilization, None);
        assert_eq!(
            reading.detail.as_deref(),
            Some("try again at Sep 9th, 2026 2:01 AM")
        );
        assert!(reading.is_notable());
        assert!(reading.describe().contains("exhausted"));
    }

    /// The refusal arrives twice, on two notifications, and neither is
    /// promised — so both are read, and the message that names no retry moment
    /// keeps the sentence that says why.
    #[test]
    fn the_error_notification_carrying_the_refusal_is_read_too() {
        let readings = parse("s-2", Provider::Codex, 2_000, CODEX_REFUSAL_ERROR);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].status, QuotaStatus::Exhausted);
        assert_eq!(
            readings[0].detail.as_deref(),
            Some("Your workspace is out of credits")
        );
    }

    /// A turn can fail for any number of reasons that say nothing about the
    /// plan, and none of them belongs in the quota log.
    #[test]
    fn a_turn_that_failed_for_another_reason_is_not_a_quota_reading() {
        let line = r#"{"method":"turn/completed","params":{"turn":{"status":"failed",
            "error":{"message":"stream disconnected","codexErrorInfo":"streamClosed"}}}}"#;
        assert!(parse("s-2", Provider::Codex, 0, line).is_empty());
    }

    /// A refusal whose prose has changed shape still records the exhaustion —
    /// the fact matters, the sentence is a nicety.
    #[test]
    fn a_refusal_without_the_familiar_prose_is_still_a_refusal() {
        let line = r#"{"error":{"codexErrorInfo":"usageLimitExceeded"}}"#;
        let readings = parse("s-2", Provider::Codex, 0, line);
        assert_eq!(readings.len(), 1);
        assert_eq!(readings[0].status, QuotaStatus::Exhausted);
        assert_eq!(readings[0].detail, None);
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

    /// A refusal is announced when it happens, and not once per retry the
    /// operator makes against a plan that is still spent.
    #[test]
    fn a_refusal_is_announced_once_while_the_plan_stays_spent() {
        let log = QuotaLog::new();
        let announced = log.observe("s-1", Provider::Codex, 1, CODEX_REFUSAL);
        assert_eq!(announced.len(), 1);
        assert!(announced[0].describe().contains("Sep 9th"));
        // The `error` notification says the same thing a millisecond later.
        assert!(log
            .observe("s-1", Provider::Codex, 2, CODEX_REFUSAL_ERROR)
            .is_empty());
        assert!(log
            .observe("s-1", Provider::Codex, 3, CODEX_REFUSAL)
            .is_empty());
        // Still logged each time, so the view shows every refusal.
        assert_eq!(log.entries().len(), 3);
    }

    /// Said once is not said forever: a window that reads empty again has
    /// turned over, so the next time the plan fills up the operator is told.
    #[test]
    fn a_plan_that_refills_and_is_spent_again_is_announced_again() {
        let log = QuotaLog::new();
        assert_eq!(
            log.observe("s-1", Provider::Codex, 1, CODEX_REFUSAL).len(),
            1
        );
        // The window turned over: Codex is running turns and reporting room.
        assert!(log
            .observe("s-1", Provider::Codex, 2, CODEX_USAGE)
            .is_empty());
        assert_eq!(
            log.observe("s-1", Provider::Codex, 3, CODEX_REFUSAL).len(),
            1
        );
        // The other provider's plan was never the one that was spent.
        assert_eq!(
            log.observe("s-2", Provider::Claude, 4, CODEX_REFUSAL).len(),
            1
        );
    }

    /// The same for an ordinary window: a five-hour limit that filled, reset,
    /// and filled again is two pieces of news, not one.
    #[test]
    fn a_window_that_turned_over_warns_again_when_it_refills() {
        let log = QuotaLog::new();
        let claude = |utilization: f64| {
            format!(
                r#"{{"rate_limit_info":{{"status":"allowed_warning","rateLimitType":"five_hour",
                   "utilization":{utilization}}}}}"#
            )
        };
        assert_eq!(
            log.observe("s-1", Provider::Claude, 1, &claude(0.91)).len(),
            1
        );
        // A plain `allowed` reading is Claude saying it has nothing to warn
        // about, which is the window having reset.
        assert!(log
            .observe("s-1", Provider::Claude, 2, CLAUDE_ALLOWED)
            .is_empty());
        assert_eq!(
            log.observe("s-1", Provider::Claude, 3, &claude(0.91)).len(),
            1
        );
    }

    /// A figure wobbling either side of the threshold is the same nearly-full
    /// window, and must not re-announce itself every turn.
    #[test]
    fn a_window_hovering_below_the_threshold_does_not_re_arm() {
        let log = QuotaLog::new();
        let codex = |percent: f64| {
            format!(
                r#"{{"params":{{"rateLimits":{{"primary":{{"usedPercent":{percent},
                   "windowDurationMins":300}}}}}}}}"#
            )
        };
        assert_eq!(
            log.observe("s-1", Provider::Codex, 1, &codex(91.0)).len(),
            1
        );
        assert!(log
            .observe("s-1", Provider::Codex, 2, &codex(88.0))
            .is_empty());
        assert!(log
            .observe("s-1", Provider::Codex, 3, &codex(92.0))
            .is_empty());
        // Genuinely empty, then filling again: that is a new window.
        assert!(log
            .observe("s-1", Provider::Codex, 4, &codex(2.0))
            .is_empty());
        assert_eq!(
            log.observe("s-1", Provider::Codex, 5, &codex(93.0)).len(),
            1
        );
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

    /// The point of keeping the log in the store: an operator who restarts the
    /// daemon, or comes back the next morning, opens knowing where the windows
    /// stood instead of an empty view.
    #[test]
    fn readings_outlive_the_daemon_that_saw_them() {
        let store = temp_dir("durable");
        {
            let log = QuotaLog::open(&store);
            log.observe("s-1", Provider::Claude, 1_000, CLAUDE_WARNING);
            log.observe("s-2", Provider::Codex, 2_000, CODEX_USAGE);
            assert_eq!(log.entries().len(), 3);
        }
        let reopened = QuotaLog::open(&store);
        let entries = reopened.entries();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].provider, Provider::Claude);
        assert_eq!(entries[0].utilization, Some(0.91));
        assert_eq!(entries[2].provider, Provider::Codex);
        // Still one log, so a reading seen after the restart joins the rest.
        reopened.observe("s-3", Provider::Claude, 3_000, CLAUDE_REJECTED);
        assert_eq!(QuotaLog::open(&store).entries().len(), 4);
        std::fs::remove_dir_all(&store).ok();
    }

    /// A log the current build cannot read is not worth refusing to start
    /// over: the providers volunteer their figures again within a turn.
    #[test]
    fn a_damaged_stored_log_costs_only_the_lines_that_are_damaged() {
        let store = temp_dir("damaged");
        std::fs::write(
            store.join(QUOTA_FILE),
            format!(
                "{}\nnot json at all\n\n{}\n",
                serde_json::to_string(&reading(Provider::Claude, "five_hour", 1_000, 0.5)).unwrap(),
                serde_json::to_string(&reading(Provider::Codex, "7d", 2_000, 0.25)).unwrap(),
            ),
        )
        .unwrap();
        let entries = QuotaLog::open(&store).entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].provider, Provider::Codex);
        std::fs::remove_dir_all(&store).ok();
    }

    /// Durable must not mean unbounded: a reading whose window has long since
    /// turned over says nothing about what is left now.
    #[test]
    fn readings_older_than_the_retention_horizon_are_dropped() {
        let mut entries: VecDeque<QuotaEvent> = VecDeque::new();
        let now = 10 * RETENTION_MS;
        entries.push_back(reading(
            Provider::Claude,
            "five_hour",
            now - RETENTION_MS - 1,
            0.1,
        ));
        entries.push_back(reading(
            Provider::Claude,
            "five_hour",
            now - RETENTION_MS + 1,
            0.2,
        ));
        entries.push_back(reading(Provider::Claude, "five_hour", now, 0.3));
        trim(&mut entries);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].utilization, Some(0.2));
    }

    /// Age is measured against the newest reading rather than the wall clock,
    /// so a log nobody has written to in a month still says what it last knew.
    #[test]
    fn an_idle_log_keeps_what_it_last_knew() {
        let mut entries: VecDeque<QuotaEvent> = VecDeque::new();
        entries.push_back(reading(Provider::Claude, "five_hour", 1_000, 0.9));
        trim(&mut entries);
        assert_eq!(entries.len(), 1);
    }

    /// The count limit drops the oldest readings, and a chatty window must not
    /// be able to push another window's last known figure out of the log.
    #[test]
    fn the_newest_reading_of_each_window_survives_a_chatty_one() {
        let mut entries: VecDeque<QuotaEvent> = VecDeque::new();
        entries.push_back(reading(Provider::Codex, "7d", 1, 0.25));
        for at_ms in 2..(CAPACITY as u64 + 2) {
            entries.push_back(reading(Provider::Claude, "five_hour", at_ms, 0.5));
        }
        trim(&mut entries);
        assert_eq!(entries.len(), CAPACITY);
        // The weekly window's only reading is the oldest entry held, and it is
        // still there — it is the only thing that answers for that window.
        assert_eq!(entries[0].window, "7d");
        // What went instead is the oldest of the window that has plenty more.
        assert_eq!(entries[1].at_ms, 3);
        assert_eq!(entries[entries.len() - 1].at_ms, CAPACITY as u64 + 1);
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
