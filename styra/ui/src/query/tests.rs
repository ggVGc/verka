use super::*;

fn deleted(query: &str) -> String {
    let mut query = query.to_owned();
    delete_word(&mut query);
    query
}

#[test]
fn a_word_ends_at_punctuation_and_whitespace() {
    assert_eq!(
        deleted("claude:claude-opus-5/high"),
        "claude:claude-opus-5/"
    );
    assert_eq!(deleted("claude:claude-opus-5/"), "claude:claude-opus-");
    assert_eq!(deleted("retry backoff  "), "retry ");
    assert_eq!(deleted("src/retry_backoff"), "src/retry_");
    assert_eq!(deleted("é·ñame"), "é·");
    assert_eq!(deleted("styra wörk- "), "styra ");
    assert_eq!(deleted("retry"), "");
    assert_eq!(deleted(""), "");
}

#[test]
fn a_prose_word_ends_at_whitespace_only() {
    let deleted = |text: &str| {
        let mut text = text.to_owned();
        delete_word_in(&mut text, Words::Prose);
        text
    };
    assert_eq!(deleted("cargo run --bin"), "cargo run ");
    assert_eq!(deleted("don't  "), "");
    assert_eq!(deleted("first line\nsecond"), "first line\n");
    assert_eq!(deleted("é ñame"), "é ");
    assert_eq!(deleted(""), "");
}

#[test]
fn keys_are_text_unless_they_are_chords() {
    let key = |code, modifiers| Edit::from_key(KeyEvent::new(code, modifiers));
    assert_eq!(
        key(KeyCode::Char('w'), KeyModifiers::NONE),
        Some(Edit::Push('w'))
    );
    assert_eq!(
        key(KeyCode::Char('W'), KeyModifiers::SHIFT),
        Some(Edit::Push('W'))
    );
    assert_eq!(
        key(KeyCode::Char('w'), KeyModifiers::CONTROL),
        Some(Edit::DeleteWord)
    );
    assert_eq!(
        key(KeyCode::Backspace, KeyModifiers::NONE),
        Some(Edit::Backspace)
    );
    assert_eq!(key(KeyCode::Char('u'), KeyModifiers::CONTROL), None);
    assert_eq!(key(KeyCode::Char('w'), KeyModifiers::ALT), None);
    assert_eq!(key(KeyCode::Enter, KeyModifiers::NONE), None);
}
