use super::*;

fn texts(notices: &Notices) -> Vec<&str> {
    notices.iter().map(|notice| notice.text.as_str()).collect()
}

#[test]
fn notices_keep_their_severity() {
    let mut notices = Notices::default();
    notices.show_level("failure", LogLevel::Error);
    notices.show_level("warning", LogLevel::Warn);
    notices.show("information");
    assert_eq!(
        notices
            .iter()
            .map(|notice| notice.level)
            .collect::<Vec<_>>(),
        vec![LogLevel::Error, LogLevel::Warn, LogLevel::Info]
    );
}

/// Styra can do several things in one keystroke, and each is worth saying.
#[test]
fn notices_accumulate_in_the_order_they_were_shown() {
    let mut notices = Notices::default();
    assert!(notices.is_empty());

    notices.show("first action");
    notices.show("second action");

    assert_eq!(texts(&notices), vec!["first action", "second action"]);
}

/// Each notice gets its own window from when it was shown, so a later one
/// is not cut short by an earlier one running out.
#[test]
fn an_expiring_notice_does_not_take_a_newer_one_with_it() {
    let mut notices = Notices::default();
    notices.show_since("old action", Duration::from_secs(5));
    notices.show("new action");

    notices.expire();

    assert_eq!(texts(&notices), vec!["new action"]);
}

/// The event loop paints a frame only when something changed, and a notice
/// leaving the screen is the only thing in its round that would know one
/// had. A round where none expired must not read as one where some did.
#[test]
fn expiry_reports_only_a_notice_that_actually_left() {
    let mut live = Notices::default();
    live.show("still up");

    assert!(!live.expire(), "nothing has run out yet");

    let mut stale = Notices::default();
    stale.show_since("long gone", Duration::from_secs(5));

    assert!(stale.expire(), "one left the screen");
    assert!(stale.is_empty());
    assert!(!stale.expire(), "and does not leave a second time");
}

/// Expiry walks from the front and stops at the first notice still within
/// its window, which is correct only because they are in order.
#[test]
fn a_notice_still_within_its_window_holds_the_queue() {
    let mut notices = Notices::default();
    notices.show_since("recent", Duration::from_secs(1));
    notices.show_since("older but behind it", Duration::from_secs(9));

    notices.expire();

    assert_eq!(
        texts(&notices),
        vec!["recent", "older but behind it"],
        "nothing behind a live notice is examined"
    );
}
