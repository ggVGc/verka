use super::*;

#[cfg(feature = "renderer")]
fn marked(line: &Line<'_>) -> Vec<String> {
    line.spans
        .iter()
        .filter(|span| span.style.bg == Some(theme::ACCENT))
        .map(|span| span.content.to_string())
        .collect()
}

#[test]
fn a_term_is_found_case_insensitively_unless_it_has_a_capital() {
    assert_eq!(matches("Retry retry", "retry"), vec![(0, 5), (6, 11)]);
    assert_eq!(matches("Retry retry", "Retry"), vec![(0, 5)]);
    assert_eq!(matches("aaaa", "aa"), vec![(0, 2), (2, 4)]);
    assert_eq!(matches("é é", "é"), vec![(0, 2), (3, 5)]);
    assert!(matches("abc", "").is_empty());
    assert!(matches("ab", "abc").is_empty());
}

#[test]
fn only_code_is_searched() {
    let text = "diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n\
                    @@ -1,2 +1,2 @@ fn f\n f\tx\n-fn\n+f\n";
    assert_eq!(
        code_lines(text),
        vec![
            None,
            None,
            None,
            None,
            Some("f    x".into()),
            Some("fn".into()),
            Some("f".into()),
        ]
    );
}

#[cfg(feature = "renderer")]
#[test]
fn the_matched_text_alone_is_marked_and_the_number_left_alone() {
    let text = "diff --git a/f b/f\n@@ -1 +1 @@\n-one one\n+none\n";
    let mut lines = vec![
        Line::from("f"),
        Line::from("┄┄┄"),
        Line::from(vec![Span::raw("1 "), Span::raw("-"), Span::raw("one one")]),
        Line::from(vec![
            Span::raw("1 "),
            Span::raw("+"),
            Span::raw("no"),
            Span::raw("ne"),
        ]),
    ];
    highlight(text, &mut lines, "one");
    assert_eq!(marked(&lines[2]), vec!["one", "one"]);
    assert_eq!(marked(&lines[3]), vec!["o", "ne"], "across spans");
    highlight(text, &mut lines, "1");
    assert_eq!(marked(&lines[2]), vec!["one", "one"], "not the number");
}
