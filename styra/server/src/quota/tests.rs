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
    assert_eq!(reading.provider, Provider::Claude);
    assert!(reading.describe().starts_with("claude plan quota"));
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
    assert!(readings
        .iter()
        .all(|reading| reading.provider == Provider::Codex));
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

#[test]
fn codex_refusal_notifications_preserve_exhaustion_without_inventing_usage() {
    for (name, line, at_ms, detail) in [
        (
            "turn refusal with retry",
            CODEX_REFUSAL,
            2_000,
            Some("try again at Sep 9th, 2026 2:01 AM"),
        ),
        (
            "error notification without retry",
            CODEX_REFUSAL_ERROR,
            2_000,
            Some("Your workspace is out of credits"),
        ),
        (
            "unfamiliar prose",
            r#"{"error":{"codexErrorInfo":"usageLimitExceeded"}}"#,
            0,
            None,
        ),
    ] {
        let readings = parse("s-2", Provider::Codex, at_ms, line);
        assert_eq!(readings.len(), 1, "{name}");
        let reading = &readings[0];
        assert_eq!(reading.provider, Provider::Codex, "{name}");
        assert_eq!(reading.window, "plan", "{name}");
        assert_eq!(reading.status, QuotaStatus::Exhausted, "{name}");
        assert_eq!(reading.utilization, None, "{name}");
        assert_eq!(reading.detail.as_deref(), detail, "{name}");
        assert!(reading.is_notable(), "{name}");
        assert!(reading.describe().contains("exhausted"), "{name}");
    }
}

/// A turn can fail for any number of reasons that say nothing about the
/// plan, and none of them belongs in the quota log.
#[test]
fn a_turn_that_failed_for_another_reason_is_not_a_quota_reading() {
    let line = r#"{"method":"turn/completed","params":{"turn":{"status":"failed",
            "error":{"message":"stream disconnected","codexErrorInfo":"streamClosed"}}}}"#;
    assert!(parse("s-2", Provider::Codex, 0, line).is_empty());
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
    let first = log
        .observe("s-1", Provider::Claude, 1, CLAUDE_WARNING)
        .announce;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].status, QuotaStatus::Warning);
    assert!(first[0].describe().contains("91%"));
    assert!(log
        .observe("s-1", Provider::Claude, 2, CLAUDE_WARNING)
        .announce
        .is_empty());
    assert!(log
        .observe("s-1", Provider::Claude, 3, CLAUDE_WARNING)
        .announce
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
        log.observe("s-1", Provider::Claude, 1, &reading(0.81))
            .announce
            .len(),
        1
    );
    // Still the same decile: already said.
    assert!(log
        .observe("s-1", Provider::Claude, 2, &reading(0.85))
        .announce
        .is_empty());
    let climbed = log
        .observe("s-1", Provider::Claude, 3, &reading(0.91))
        .announce;
    assert_eq!(climbed.len(), 1);
    assert_eq!(climbed[0].status, QuotaStatus::Warning);
    assert_eq!(climbed[0].utilization, Some(0.91));
    assert!(log
        .observe("s-1", Provider::Claude, 4, &reading(0.95))
        .announce
        .is_empty());
    assert!(log
        .observe("s-1", Provider::Claude, 5, &reading(0.99))
        .announce
        .is_empty());
    // A full window is a different status, so it is announced again.
    let exhausted = log
        .observe("s-1", Provider::Claude, 6, CLAUDE_REJECTED)
        .announce;
    assert_eq!(exhausted.len(), 1);
    assert_eq!(exhausted[0].status, QuotaStatus::Exhausted);
    let entries = log.entries();
    assert_eq!(entries.len(), 6);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.utilization)
            .collect::<Vec<_>>(),
        vec![
            Some(0.81),
            Some(0.85),
            Some(0.91),
            Some(0.95),
            Some(0.99),
            None
        ]
    );
}

/// A refusal is announced when it happens, and not once per retry the
/// operator makes against a plan that is still spent.
#[test]
fn a_refusal_is_announced_once_while_the_plan_stays_spent() {
    let log = QuotaLog::new();
    let announced = log
        .observe("s-1", Provider::Codex, 1, CODEX_REFUSAL)
        .announce;
    assert_eq!(announced.len(), 1);
    assert!(announced[0].describe().contains("Sep 9th"));
    // The `error` notification says the same thing a millisecond later.
    assert!(log
        .observe("s-1", Provider::Codex, 2, CODEX_REFUSAL_ERROR)
        .announce
        .is_empty());
    assert!(log
        .observe("s-1", Provider::Codex, 3, CODEX_REFUSAL)
        .announce
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
        log.observe("s-1", Provider::Codex, 1, CODEX_REFUSAL)
            .announce
            .len(),
        1
    );
    // The window turned over: Codex is running turns and reporting room.
    assert!(log
        .observe("s-1", Provider::Codex, 2, CODEX_USAGE)
        .announce
        .is_empty());
    assert_eq!(
        log.observe("s-1", Provider::Codex, 3, CODEX_REFUSAL)
            .announce
            .len(),
        1
    );
    // The other provider's plan was never the one that was spent.
    assert_eq!(
        log.observe("s-2", Provider::Claude, 4, CODEX_REFUSAL)
            .announce
            .len(),
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
        log.observe("s-1", Provider::Claude, 1, &claude(0.91))
            .announce
            .len(),
        1
    );
    // A plain `allowed` reading is Claude saying it has nothing to warn
    // about, which is the window having reset.
    assert!(log
        .observe("s-1", Provider::Claude, 2, CLAUDE_ALLOWED)
        .announce
        .is_empty());
    assert_eq!(
        log.observe("s-1", Provider::Claude, 3, &claude(0.91))
            .announce
            .len(),
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
        log.observe("s-1", Provider::Codex, 1, &codex(91.0))
            .announce
            .len(),
        1
    );
    assert!(log
        .observe("s-1", Provider::Codex, 2, &codex(88.0))
        .announce
        .is_empty());
    assert!(log
        .observe("s-1", Provider::Codex, 3, &codex(92.0))
        .announce
        .is_empty());
    // Genuinely empty, then filling again: that is a new window.
    assert!(log
        .observe("s-1", Provider::Codex, 4, &codex(2.0))
        .announce
        .is_empty());
    assert_eq!(
        log.observe("s-1", Provider::Codex, 5, &codex(93.0))
            .announce
            .len(),
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
    let announced = log.observe("s-1", Provider::Codex, 1, line).announce;
    assert_eq!(announced.len(), 2);
    assert_eq!(announced[0].window, "1h");
    assert_eq!(announced[1].window, "7d");
}

/// Two providers can name a window alike — and even when they do not, one
/// account filling up says nothing about the other, so each is announced
/// on its own.
#[test]
fn the_same_window_name_on_two_providers_is_announced_once_each() {
    let log = QuotaLog::new();
    let line = r#"{"rate_limit_info":{"status":"allowed_warning",
            "rateLimitType":"5h","utilization":0.91}}"#;
    assert_eq!(
        log.observe("s-1", Provider::Claude, 1, line).announce.len(),
        1
    );
    let codex = log.observe("s-2", Provider::Codex, 2, line).announce;
    assert_eq!(codex.len(), 1);
    assert_eq!(codex[0].provider, Provider::Codex);
    // Each provider has now been reported, so neither repeats.
    assert!(log
        .observe("s-1", Provider::Claude, 3, line)
        .announce
        .is_empty());
    assert!(log
        .observe("s-2", Provider::Codex, 4, line)
        .announce
        .is_empty());
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

// --- Windows coming back -------------------------------------------------

/// The Claude rejection above resets at this second, so a reset is due
/// three minutes past it.
const REJECTED_RESET_MS: u64 = 1_788_290_400_000;
const REJECTED_DUE_MS: u64 = REJECTED_RESET_MS + RESET_GRACE_MS;

/// A rejection is a different kind of fact from a usage figure: it says an
/// interaction has just been stopped, and by which window.
#[test]
fn a_rejection_is_reported_as_the_reason_work_stopped() {
    let log = QuotaLog::new();

    let observed = log.observe("s-1", Provider::Claude, 1, CLAUDE_REJECTED);

    let rejected = observed.rejected.expect("the line refused the work");
    assert_eq!(rejected.window, "five_hour");
    assert_eq!(rejected.resets_at_ms, Some(REJECTED_RESET_MS));
    // A window merely filling up has stopped nothing.
    assert!(log
        .observe("s-1", Provider::Claude, 2, CLAUDE_WARNING)
        .rejected
        .is_none());
}

/// The point of the watch list: the window that refused work is handed
/// back once it has turned over — a few minutes past the reported minute,
/// since the provider is the one rounding it.
#[test]
fn a_refused_window_comes_back_a_few_minutes_after_its_reset() {
    let log = QuotaLog::new();
    log.observe("s-1", Provider::Claude, 1, CLAUDE_REJECTED);
    assert!(log.is_awaiting_reset());

    assert!(
        log.resets(REJECTED_RESET_MS).is_empty(),
        "the reported minute alone is too early to send work at"
    );
    let due = log.resets(REJECTED_DUE_MS);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].provider, Provider::Claude);
    assert_eq!(due[0].window, "five_hour");
    assert_eq!(due[0].at_ms, REJECTED_DUE_MS);
    assert!(!log.is_awaiting_reset());
    // Timer polling must never hand the same reset out twice.
    assert!(log.resets(REJECTED_DUE_MS).is_empty());
    assert!(log.resets(REJECTED_DUE_MS + RETENTION_MS).is_empty());
}

/// A window rejecting turn after turn is still one window with one reset,
/// and the latest word on when it turns over is the one to wait for.
#[test]
fn repeated_rejections_of_one_window_wait_for_one_reset() {
    let log = QuotaLog::new();
    log.observe("s-1", Provider::Claude, 1, CLAUDE_REJECTED);
    let later = CLAUDE_REJECTED.replace("1788290400", "1788290700");

    log.observe("s-2", Provider::Claude, 2, &later);

    assert!(
        log.resets(REJECTED_DUE_MS).is_empty(),
        "the later reset is the one being waited for"
    );
    let due = log.resets(REJECTED_DUE_MS + 300_000);
    assert_eq!(due.len(), 1);
}

/// The provider saying a watched window is allowed again outranks the
/// reset it predicted earlier: work is going through, so whatever is being
/// held back need not wait for a clock.
#[test]
fn a_window_reporting_itself_allowed_again_comes_back_at_once() {
    let log = QuotaLog::new();
    log.observe("s-1", Provider::Claude, 1, CLAUDE_REJECTED);

    log.observe("s-1", Provider::Claude, 5_000, CLAUDE_ALLOWED);

    let due = log.resets(5_000);
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].at_ms, 5_000);
}

/// An unwatched window reporting itself allowed is the ordinary case and
/// starts nothing: there was nothing waiting for it.
#[test]
fn an_allowed_window_nobody_is_waiting_for_announces_nothing() {
    let log = QuotaLog::new();

    log.observe("s-1", Provider::Claude, 1, CLAUDE_ALLOWED);

    assert!(!log.is_awaiting_reset());
    assert!(log.resets(u64::MAX).is_empty());
}

/// Without a reported reset there is no minute to come back at, and a
/// guessed one would call a window usable while it is still refusing.
#[test]
fn a_rejection_naming_no_reset_is_not_waited_for() {
    let log = QuotaLog::new();
    let no_reset = r#"{"rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}"#;

    let observed = log.observe("s-1", Provider::Claude, 1, no_reset);

    assert!(observed.rejected.is_some(), "the work was still refused");
    assert!(!log.is_awaiting_reset());
    assert!(log.resets(u64::MAX).is_empty());
}

/// Two accounts, two subscriptions: each window comes back on its own, and
/// the reset says whose plan it is so the wrong sessions are not woken.
#[test]
fn each_provider_window_comes_back_on_its_own() {
    let log = QuotaLog::new();
    let codex = r#"{"params":{"rate_limits":{
            "primary":{"used_percent":100.0,"window_minutes":60,"resets_at":1788280000},
            "secondary":{"used_percent":100.0,"window_minutes":10080,"resets_at":1788380000}}}}"#;
    log.observe("s-1", Provider::Claude, 1, CLAUDE_REJECTED);
    log.observe("s-2", Provider::Codex, 2, codex);

    let first = log.resets(1_788_280_000_000 + RESET_GRACE_MS);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].provider, Provider::Codex);
    assert_eq!(first[0].window, "1h");
    // The other two are still refusing, each until its own minute.
    let rest = log.resets(1_788_380_000_000 + RESET_GRACE_MS);
    assert_eq!(rest.len(), 2);
    assert_eq!(rest[0].provider, Provider::Claude);
    assert_eq!(rest[1].window, "7d");
}

/// A rejection this run never saw belongs to work that died with the run
/// that did: there is nothing left for its reset to come back to.
#[test]
fn a_stored_rejection_is_not_waited_for_by_the_next_run() {
    let store = temp_dir("reopened-rejection");
    {
        let log = QuotaLog::open(&store);
        log.observe("s-1", Provider::Claude, 1, CLAUDE_REJECTED);
        assert!(log.is_awaiting_reset());
    }

    let reopened = QuotaLog::open(&store);

    assert!(!reopened.is_awaiting_reset());
    assert!(reopened.resets(u64::MAX).is_empty());
    std::fs::remove_dir_all(&store).ok();
}

#[test]
fn window_lengths_read_as_an_operator_would_say_them() {
    assert_eq!(humanize_window(60), "1h");
    assert_eq!(humanize_window(300), "5h");
    assert_eq!(humanize_window(1_440), "1d");
    assert_eq!(humanize_window(10_080), "7d");
    assert_eq!(humanize_window(90), "90m");
}
