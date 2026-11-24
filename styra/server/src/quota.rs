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
//!
//! The other half of a rejection is when it stops being one. A refused window
//! reports the minute it turns over, and this is the one place that hears every
//! such reading, so this is where a window coming back is noticed: a rejection
//! puts the window on a watch list ([`QuotaLog::observe`] again, through
//! [`Observed::rejected`]), and [`QuotaLog::resets`] hands back the windows
//! whose moment has come, once each. What anyone does with that — Styra resumes
//! the interactions the window stopped — is the caller's; deciding *that a
//! window is usable again* is this module's, because the readings it is decided
//! from all arrive here anyway.

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

/// How long past a window's reported reset it is called usable again.
///
/// The providers report the minute a window turns over, and the request that
/// lands exactly on it is the one most likely to be refused for being early:
/// the figure is theirs to round, and whoever is waiting on this reset is not
/// watching. A few minutes late costs nothing; a few seconds early costs the
/// whole wait again.
const RESET_GRACE_MS: u64 = 3 * 60 * 1_000;

/// A provider window that has come back: it refused work, and the moment it
/// said it would turn over has passed.
///
/// Handed out once per rejection (see [`QuotaLog::resets`]), so acting on one
/// is safe to do without asking whether it has already been acted on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowReset {
    pub provider: Provider,
    /// The window as the provider names it, matching [`QuotaEvent::window`].
    pub window: String,
    /// When it was called usable again, which is the reset the provider
    /// reported plus [`RESET_GRACE_MS`] — or the moment the provider itself
    /// said the window was allowing work again, whichever came first.
    pub at_ms: u64,
}

/// What one observed line told the log.
#[derive(Debug, Default, PartialEq)]
pub struct Observed {
    /// The readings worth putting in front of the operator now; see
    /// [`QuotaLog::observe`].
    pub announce: Vec<QuotaEvent>,
    /// The window that refused the work, when this line was a rejection.
    ///
    /// Kept apart from `announce` because it is a different kind of fact: the
    /// others are figures about how full a window is, and this is the reason
    /// an interaction has just stopped. It is reported on every rejection, not
    /// only the first, since it is the caller's business whose work was
    /// refused rather than whether the news is new.
    pub rejected: Option<QuotaEvent>,
    /// Whether the line carried a reading that is not a refusal — any window
    /// of this provider reporting room, or merely filling up.
    ///
    /// A reading exists because a turn ran and the provider answered it, so
    /// this is the provider saying it is serving: it is what retires a refusal
    /// recorded earlier, which nothing else ever contradicts by name.
    pub serving: bool,
}

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
    /// For each window that has refused work, the moment it is expected to
    /// take it again: the reset it reported plus [`RESET_GRACE_MS`], brought
    /// forward if the provider itself says the window is allowing work again.
    /// Drained by [`QuotaLog::resets`], so an entry here is a reset nobody has
    /// been told about yet.
    awaiting_reset: Mutex<HashMap<(Provider, String), u64>>,
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
            // Nor this, for the same reason at its strongest: the work a
            // previous run's rejection stopped died with that run, so there is
            // nothing left for its reset to come back to.
            awaiting_reset: Mutex::default(),
        }
    }

    /// Read any quota figures out of one verbatim agent line, record them all,
    /// and say what the caller has to act on: the readings the operator should
    /// be told about now, and the rejection the line carried, if any.
    ///
    /// A line carrying no quota figures — nearly all of them — costs one
    /// substring scan and nothing else.
    pub fn observe(&self, session_id: &str, provider: Provider, at_ms: u64, raw: &str) -> Observed {
        let readings = parse(session_id, provider, at_ms, raw);
        if readings.is_empty() {
            return Observed::default();
        }
        let mut observed = Observed::default();
        for reading in readings {
            if self.worth_reporting(&reading) {
                observed.announce.push(reading.clone());
            }
            self.note_availability(&reading);
            if reading.status == QuotaStatus::Exhausted {
                observed.rejected = Some(reading.clone());
            } else {
                observed.serving = true;
            }
            self.record(reading);
        }
        observed
    }

    /// Keep the watch list current with one reading: a rejection puts its
    /// window on it, and a window reporting that it is allowing work again
    /// takes its own reset off the clock.
    ///
    /// A rejection that names no reset is not watched. The whole of this is
    /// "tell me when it comes back", and without the minute it comes back
    /// there is nothing to tell — a guessed reset would announce a window as
    /// usable while it is still refusing, which is worse than not announcing
    /// it at all. The providers do report one with a rejection.
    fn note_availability(&self, reading: &QuotaEvent) {
        let mut awaiting = self
            .awaiting_reset
            .lock()
            .expect("quota reset lock poisoned");
        let key = (reading.provider, reading.window.clone());
        match reading.status {
            QuotaStatus::Exhausted => {
                let Some(resets_at_ms) = reading.resets_at_ms else {
                    return;
                };
                // A second rejection of the same window replaces the first
                // rather than queueing another reset: it is one window, and
                // what matters is the latest word on when it turns over.
                awaiting.insert(key, resets_at_ms + RESET_GRACE_MS);
            }
            // The provider saying a watched window is allowed again outranks
            // the reset it predicted earlier: whatever the clock says, work is
            // going through, so the wait is over now.
            QuotaStatus::Allowed | QuotaStatus::Warning => {
                if let Some(due) = awaiting.get_mut(&key) {
                    *due = (*due).min(reading.at_ms);
                }
            }
        }
    }

    /// The watched windows that have come back by `now_ms`, oldest reset
    /// first, taken off the watch list as they are handed over.
    ///
    /// Draining is the point: this is called on a timer, and a reset announced
    /// twice would send the same held-back work twice. What a caller does with
    /// one is its own business — see [`WindowReset`].
    pub fn resets(&self, now_ms: u64) -> Vec<WindowReset> {
        let mut awaiting = self
            .awaiting_reset
            .lock()
            .expect("quota reset lock poisoned");
        let mut due: Vec<WindowReset> = awaiting
            .iter()
            .filter(|(_, at_ms)| **at_ms <= now_ms)
            .map(|((provider, window), at_ms)| WindowReset {
                provider: *provider,
                window: window.clone(),
                at_ms: *at_ms,
            })
            .collect();
        for reset in &due {
            awaiting.remove(&(reset.provider, reset.window.clone()));
        }
        due.sort_by(|a, b| a.at_ms.cmp(&b.at_ms).then_with(|| a.window.cmp(&b.window)));
        due
    }

    /// Whether any window is being waited on, so a caller polling
    /// [`Self::resets`] can say whether there is anything to wait for.
    pub fn is_awaiting_reset(&self) -> bool {
        !self
            .awaiting_reset
            .lock()
            .expect("quota reset lock poisoned")
            .is_empty()
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
mod tests;
