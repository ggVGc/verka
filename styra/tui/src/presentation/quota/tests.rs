use super::super::test_support;
use styra_protocol::{agent::Provider, LogLevel, QuotaEvent, QuotaStatus};
fn reading(status: QuotaStatus, utilization: Option<f64>) -> QuotaEvent {
    QuotaEvent {
        at_ms: 1_000,
        session_id: "s1".into(),
        provider: Provider::Claude,
        window: "five_hour".into(),
        status,
        utilization,
        resets_at_ms: None,
        detail: None,
    }
}
#[test]
fn announced_reading_updates_application_notices_and_log() {
    let mut app = test_support::app("s1");
    app.note_quota(reading(QuotaStatus::Warning, Some(0.91)));
    assert_eq!(app.quota.iter().count(), 1);
    assert!(app
        .notices
        .iter()
        .any(|notice| notice.text.contains("91% used")));
    assert_eq!(app.log.newest().unwrap().level, LogLevel::Warn);
}
#[test]
fn exhausted_reading_is_logged_as_an_error() {
    let mut app = test_support::app("s1");
    app.note_quota(reading(QuotaStatus::Exhausted, None));
    assert_eq!(app.log.newest().unwrap().level, LogLevel::Error);
}
