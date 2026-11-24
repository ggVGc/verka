use super::*;
fn marked(a: &str, b: &str) -> (String, String) {
    let m = changes(a, b, &mut 2_000_000).unwrap();
    (
        a.chars()
            .zip(m.old)
            .filter_map(|(c, yes)| yes.then_some(c))
            .collect(),
        b.chars()
            .zip(m.new)
            .filter_map(|(c, yes)| yes.then_some(c))
            .collect(),
    )
}
#[test]
fn refines_words_numbers_and_unicode() {
    assert_eq!(
        marked("timeout_ms = 30", "timeout_secs = 60"),
        ("m3".into(), "sec6".into())
    );
    assert_eq!(
        marked("let café = 1;", "let café = 2;"),
        ("1".into(), "2".into())
    );
    assert_eq!(
        marked("call(foo, bar)", "call(foo, baz)"),
        ("r".into(), "z".into())
    );
}
#[test]
fn rejects_unrelated_and_bounds_work() {
    assert!(changes("foo", "bar", &mut 10000).is_none());
    assert!(changes("let a = 1", "let a = 2", &mut 1).is_none());
    assert!(changes(&"a".repeat(5000), "a", &mut 10000).is_none());
}
#[test]
fn pairs_past_insertions_and_keeps_line_bands() {
    let text = "@@ -1 +1,2 @@\n-let timeout = 30;\n+unrelated();\n+let timeout = 60;\n";
    let mut lines = diff_block_lines(text, None, false);
    let original = lines.clone();
    highlight_words(text, &mut lines);
    assert!(lines[2]
        .spans
        .iter()
        .all(|s| s.style.bg != Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
    assert!(lines[3]
        .spans
        .iter()
        .any(|s| s.content == "6" && s.style.bg == Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
    for index in [1, 3] {
        let word = lines[index]
            .spans
            .iter()
            .find(|s| {
                [
                    Some(theme::DIFF_WORD_ADDED_BACKGROUND),
                    Some(theme::DIFF_WORD_REMOVED_BACKGROUND),
                ]
                .contains(&s.style.bg)
            })
            .unwrap();
        let original_style = original[index].spans.last().unwrap().style;
        if word.style.bg == Some(theme::DIFF_WORD_REMOVED_BACKGROUND) {
            let lifted = blended_style(original_style, theme::DIFF_WORD_REMOVED_FOREGROUND_LIFT, 4);
            assert_eq!(word.style.fg, lifted.fg);
        } else {
            assert_eq!(word.style.fg, original_style.fg);
        }
        assert_eq!(word.style.add_modifier, original_style.add_modifier);
        assert_eq!(word.style.sub_modifier, original_style.sub_modifier);
    }
    fill_changed_rows(&mut lines, 80);
    assert_eq!(
        lines[3].spans.last().unwrap().style.bg,
        Some(theme::DIFF_ADDED_BACKGROUND)
    );
}
#[test]
fn whitespace_tabs_and_hunk_boundaries() {
    assert_eq!(marked("  ", " "), (" ".into(), "".into()));
    let text =
        "@@ -1 +1 @@\n-\tlet a = 1;\n+ let a = 1;\n@@ -3 +3 @@\n-hello\n@@ -5 +5 @@\n+hello!\n";
    let mut lines = diff_block_lines(text, None, false);
    highlight_words(text, &mut lines);
    assert!(lines[1].spans.iter().any(|s| s.content.contains('→')));
    assert!(lines[4]
        .spans
        .iter()
        .all(|s| s.style.bg != Some(theme::DIFF_WORD_REMOVED_BACKGROUND)));
    assert!(lines[6]
        .spans
        .iter()
        .all(|s| s.style.bg != Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
}
