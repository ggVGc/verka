use super::*;

/// A restored box is already saved; editing it is what makes it unsaved,
/// and a stored state that names no box still opens one.
#[test]
fn restoring_tracks_what_the_server_holds() {
    let composer = Composer::default();
    assert_eq!(composer.unsaved(), None);

    let mut composer = Composer::restore(ComposerState {
        parts: vec!["a".into(), "b".into()],
        focused: 7,
        history: vec!["sent".into()],
    });
    assert_eq!(composer.focused(), 1);
    assert_eq!(composer.unsaved(), None);
    composer.char('!');
    let state = composer.unsaved().unwrap();
    assert_eq!(state.parts, ["a", "b!"]);
    composer.mark_saved(state);
    assert_eq!(composer.unsaved(), None);
    // Sending changes the history as well as the boxes.
    assert_eq!(composer.take(), Some("a\n\nb!".into()));
    assert_eq!(composer.unsaved().unwrap().history, ["sent", "a\n\nb!"]);

    let empty = Composer::restore(ComposerState::default());
    assert_eq!(empty.parts(), [""]);
}

#[test]
fn typing_edits_the_buffer_and_sending_empties_it() {
    let mut composer = Composer::default();
    composer.char('h');
    composer.char('i');
    composer.newline();
    composer.char('!');
    composer.backspace();
    assert_eq!(composer.text(), "hi\n");
    assert_eq!(composer.take(), Some("hi".into()));
    assert!(composer.text().is_empty());
    assert_eq!(composer.take(), None);
}

#[test]
fn delete_word_removes_the_trailing_word_readline_style() {
    let mut composer = Composer::default();
    composer.set("fix the flaky test".into());
    composer.delete_word();
    assert_eq!(composer.text(), "fix the flaky ");
    composer.delete_word();
    assert_eq!(composer.text(), "fix the ");
    composer.set("one".into());
    composer.delete_word();
    assert!(composer.text().is_empty());
    // And on an empty buffer it is a no-op rather than an underflow.
    composer.delete_word();
    assert!(composer.text().is_empty());
}

/// An inserted path is a word: it never runs into the word before it, and
/// it never doubles a space the operator already typed.
#[test]
fn inserting_separates_what_it_appends() {
    let mut composer = Composer::default();
    composer.insert("/srv/data/notes.txt");
    assert_eq!(composer.text(), "/srv/data/notes.txt");
    composer.set("summarize".into());
    composer.insert("/srv/data/notes.txt");
    assert_eq!(composer.text(), "summarize /srv/data/notes.txt");
    composer.set("summarize ".into());
    composer.insert("/srv/data/notes.txt");
    assert_eq!(composer.text(), "summarize /srv/data/notes.txt");
}

/// Walking back through history keeps the half-typed message, so `Down`
/// returns to it rather than to an empty box.
#[test]
fn history_walks_back_and_returns_to_the_draft() {
    let mut composer = Composer::default();
    composer.set("first".into());
    composer.take();
    composer.set("second".into());
    composer.take();

    composer.set("draft".into());
    composer.history_previous();
    assert_eq!(composer.text(), "second");
    composer.history_previous();
    assert_eq!(composer.text(), "first");
    // Already at the oldest: staying there beats wrapping around.
    composer.history_previous();
    assert_eq!(composer.text(), "first");

    composer.history_next();
    assert_eq!(composer.text(), "second");
    composer.history_next();
    assert_eq!(composer.text(), "draft");
    // Past the newest there is nothing to return to a second time.
    composer.history_next();
    assert_eq!(composer.text(), "draft");
}

/// Typing anything abandons the walk, so the next `Up` starts over from the
/// newest entry with the new text as the draft.
#[test]
fn typing_ends_a_history_walk() {
    let mut composer = Composer::default();
    composer.set("first".into());
    composer.take();

    composer.history_previous();
    assert_eq!(composer.text(), "first");
    composer.char('!');
    composer.history_previous();
    assert_eq!(composer.text(), "first");
    composer.history_next();
    assert_eq!(composer.text(), "first!");
}
/// Each box is typed separately; sending joins the non-blank ones as
/// paragraphs, in order, and leaves one empty box behind.
#[test]
fn boxes_are_sent_together_as_paragraphs() {
    let mut composer = Composer::default();
    composer.set("fix the test ".into());
    composer.add_part();
    assert_eq!(composer.focused(), 1);
    assert!(composer.text().is_empty());
    composer.add_part();
    composer.insert("it fails on CI");
    assert_eq!(composer.message(), "fix the test\n\nit fails on CI");
    assert_eq!(
        composer.take(),
        Some("fix the test\n\nit fails on CI".into())
    );
    assert_eq!(composer.parts().len(), 1);
    assert!(composer.text().is_empty());
}

/// A new box goes straight after the current one, not at the end.
#[test]
fn a_new_box_follows_the_current_one() {
    let mut composer = Composer::default();
    composer.set("first".into());
    composer.add_part();
    composer.insert("third");
    assert!(composer.start_choosing());
    composer.choose_previous();
    composer.stop_choosing();
    composer.add_part();
    composer.insert("second");
    assert_eq!(composer.parts(), ["first", "second", "third"]);
}

#[test]
fn pastes_create_separate_boxes_after_the_focused_box() {
    let mut composer = Composer::default();
    composer.set("instruction".into());
    composer.add_part();
    composer.insert("constraint");
    composer.start_choosing();
    composer.choose_previous();

    composer.paste("  log\n世界\n");
    assert_eq!(
        composer.parts(),
        ["instruction", "  log\n世界\n", "", "constraint"]
    );
    // Typing resumes in a fresh box below the paste, not inside it.
    assert_eq!(composer.focused(), 2);
    assert!(!composer.choosing());
    composer.paste("another log");
    composer.char('!');
    assert_eq!(
        composer.parts(),
        [
            "instruction",
            "  log\n世界\n",
            "",
            "another log",
            "!",
            "constraint"
        ]
    );
    assert_eq!(
        composer.take(),
        Some("instruction\n\nlog\n世界\n\nanother log\n\n!\n\nconstraint".into())
    );
}

#[test]
fn pasting_into_an_empty_editor_creates_a_box_and_empty_paste_is_ignored() {
    let mut composer = Composer::default();
    composer.paste("");
    assert_eq!(composer.parts(), [""]);
    assert_eq!(composer.unsaved(), None);
    composer.paste("text");
    assert_eq!(composer.parts(), ["", "text", ""]);
    assert_eq!(composer.focused(), 2);
    assert_eq!(composer.unsaved().unwrap().parts, ["", "text", ""]);
}

/// Choosing needs something to choose between, and stays within the boxes.
#[test]
fn choosing_moves_between_boxes_and_removes_them() {
    let mut composer = Composer::default();
    assert!(!composer.start_choosing());
    assert!(!composer.choosing());

    composer.set("a".into());
    composer.add_part();
    composer.insert("b");
    assert!(composer.start_choosing());
    composer.choose_next();
    assert_eq!(composer.focused(), 1);
    composer.choose_previous();
    composer.choose_previous();
    assert_eq!(composer.focused(), 0);

    composer.remove_chosen();
    assert_eq!(composer.parts(), ["b"]);
    // With one box left there is nothing to choose between.
    assert!(!composer.choosing());
    composer.remove_chosen();
    assert_eq!(composer.parts(), ["b"]);
}

/// History recalls into the box being typed in, leaving the others be.
#[test]
fn history_recalls_into_the_current_box() {
    let mut composer = Composer::default();
    composer.set("earlier".into());
    composer.take();
    composer.set("context".into());
    composer.add_part();
    composer.history_previous();
    assert_eq!(composer.parts(), ["context", "earlier"]);
    composer.history_next();
    assert_eq!(composer.parts(), ["context", ""]);
}
