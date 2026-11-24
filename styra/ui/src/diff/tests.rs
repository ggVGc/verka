use super::*;

fn text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn hunk_cuts_show_context_without_coordinates() {
    for compact in [false, true] {
        let lines = diff_body_lines(
            "@@ -805,6 +847,7 @@ mod tests {\n+added\n@@ -900 +943 @@\n+next",
            None,
            compact,
        );
        assert_eq!(text(&lines[0]), "mod tests {");
        assert_eq!(text(&lines[1]), "847 +added");
        assert_eq!(text(&lines[2]), "┄┄┄");
        assert_eq!(text(&lines[3]), "943 +next");
    }
}

#[test]
fn signs_are_green_and_red_and_the_code_is_highlighted() {
    let lines = diff_body_lines(
        "@@ edit @@\n-fn old() -> u32 { 1 }\n+fn new() -> u32 { 2 }",
        Some("src/lib.rs"),
        false,
    );

    assert_eq!(lines.len(), 3);
    assert_eq!(lines[1].spans[0].content.as_ref(), "-");
    assert_eq!(
        lines[1].spans[0].style.fg,
        blended_style(
            faded_style(Style::default().fg(theme::ERROR), 3),
            theme::DIFF_REMOVED_FOREGROUND_TINT,
            4,
        )
        .fg
    );
    assert_eq!(lines[2].spans[0].content.as_ref(), "+");
    assert_eq!(lines[2].spans[0].style.fg, Some(theme::SUCCESS));
    assert!(lines[2].spans.len() > 3, "tokenized: {:?}", lines[2].spans);
    assert_eq!(text(&lines[2]), "+fn new() -> u32 { 2 }");
}

/// Padding a changed row to the panel's width fills it without pushing it
/// onto a second row where the preview wraps.
#[test]
fn a_filled_row_still_takes_one_row() {
    use ratatui::widgets::{Paragraph, Wrap};
    let mut lines = diff_body_lines("@@ edit @@\n-old\n+new", Some("notes.unknown"), false);
    fill_changed_rows(&mut lines, 30);

    assert_eq!(lines[1].width(), 30);
    assert_eq!(lines[2].width(), 30);
    assert_eq!(
        lines[0].width(),
        "edit".chars().count(),
        "headers are left alone"
    );
    let rows = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .line_count(30);
    assert_eq!(rows, 3);
}

/// Changed rows carry a faint band of their sign's hue; context and
/// headers do not. Code with no grammar is banded the same way.
#[test]
fn changed_rows_have_a_background_hint_and_context_does_not() {
    for path in [
        "src/lib.rs",
        "notes.unknown",
        "lib/example.ex",
        "test/example.exs",
    ] {
        let lines = diff_body_lines("@@ edit @@\n same\n-old\n+new", Some(path), false);

        assert_eq!(lines[0].style.bg, None, "{path}: header");
        assert_eq!(lines[1].style.bg, None, "{path}: context");
        assert_eq!(
            lines[2].style.bg,
            Some(theme::DIFF_REMOVED_BACKGROUND),
            "{path}"
        );
        assert_eq!(
            lines[3].style.bg,
            Some(theme::DIFF_ADDED_BACKGROUND),
            "{path}"
        );
    }
}

#[test]
fn elixir_sources_and_scripts_are_highlighted_with_diff_cues() {
    for path in ["lib/example.ex", "test/example_test.exs"] {
        let lines = diff_body_lines(
            "@@ -1,2 +1,2 @@\n-def old, do: :old\n+def new, do: :new\n # comment",
            Some(path),
            false,
        );
        assert_eq!(text(&lines[1]), "1 -def old, do: :old");
        assert_eq!(text(&lines[2]), "1 +def new, do: :new");
        assert_eq!(text(&lines[3]), "2  # comment");
        assert_eq!(
            lines[1].spans[1].style.fg,
            blended_style(
                faded_style(Style::default().fg(theme::ERROR), 3),
                theme::DIFF_REMOVED_FOREGROUND_TINT,
                4,
            )
            .fg
        );
        assert_eq!(lines[2].spans[1].style.fg, Some(theme::SUCCESS));
        for line in &lines[1..=2] {
            assert!(line.spans.len() > 4, "tokenized {path}: {:?}", line.spans);
            assert!(line.spans[2..].iter().any(|span| {
                span.content.contains("def")
                    && span.style.fg != Some(theme::SUCCESS)
                    && span.style.fg != Some(theme::ERROR)
            }));
        }
    }
}

#[test]
fn elixir_in_file_headers_preserves_blank_lines_and_multiline_strings() {
    let lines = diff_body_lines(
            "diff --git a/lib/example.ex b/lib/example.ex\n@@\n+message = \"\"\"\n+hello\n+\n+world\n+\"\"\"",
            None,
            true,
        );
    let texts: Vec<String> = lines.iter().map(text).collect();
    assert_eq!(
        texts,
        [
            "lib/example.ex",
            "┄┄┄",
            "+message = \"\"\"",
            "+hello",
            "+",
            "+world",
            "+\"\"\""
        ]
    );
    for index in [3, 5] {
        assert_eq!(lines[index].spans[0].style.fg, Some(theme::SUCCESS));
        assert_eq!(lines[index].spans[1].style.fg, Some(crate::palette::PEACH));
    }
}

/// Each file in a multi-file diff is highlighted as what it is, which only
/// its own header says.
#[test]
fn the_language_follows_the_file_headers() {
    let lines = diff_body_lines(
        "diff --git a/notes.txt b/notes.txt\n@@\n+plain words here\n\
             diff --git a/src/a.rs b/src/a.rs\n@@\n+let x = \"s\";",
        None,
        true,
    );

    let texts: Vec<String> = lines.iter().map(text).collect();
    assert_eq!(
        texts,
        [
            "notes.txt",
            "┄┄┄",
            "+plain words here",
            "src/a.rs",
            "┄┄┄",
            "+let x = \"s\";"
        ]
    );
    // Text has no grammar, so it keeps the plain green line.
    assert_eq!(lines[2].spans[1].style.fg, Some(theme::SUCCESS));
    assert!(lines[5].spans.len() > 3, "{:?}", lines[5].spans);
}

#[test]
fn compact_drops_context_and_file_metadata() {
    let diff = "diff --git a/f.rs b/f.rs\nindex 1..2 100644\n--- a/f.rs\n+++ b/f.rs\n\
                    @@ -1,3 +1,3 @@\n fn keep() {}\n-fn gone() {}\n+fn added() {}";
    let full: Vec<String> = diff_body_lines(diff, None, false)
        .iter()
        .map(text)
        .collect();
    assert_eq!(full.len(), 8);

    let compact: Vec<String> = diff_body_lines(diff, None, true).iter().map(text).collect();
    assert_eq!(
        compact,
        ["f.rs", "┄┄┄", "2 -fn gone() {}", "2 +fn added() {}"],
        "the hidden context still counts toward the numbers"
    );
}

#[test]
fn diff_line_numbers_follow_hunk_positions_and_disappear_without_them() {
    for (case, diff, expected) in [
        (
            "mixed hunks leave an unpositioned hunk blank",
            "@@ -8,3 +8,4 @@\n a\n-b\n+c\n+d\n e\n@@ edit @@\n+f",
            vec![
                "┄┄┄",
                " 8  a",
                " 9 -b",
                " 9 +c",
                "10 +d",
                "11  e",
                "edit",
                "   +f",
            ],
        ),
        (
            "no positions means no number column",
            "@@ edit @@\n-a\n+b",
            vec!["edit", "-a", "+b"],
        ),
    ] {
        let texts: Vec<String> = diff_body_lines(diff, None, false)
            .iter()
            .map(text)
            .collect();
        assert_eq!(texts, expected, "{case}");
    }
}

#[test]
fn a_removed_line_that_looks_like_a_file_marker_stays_code() {
    let lines = diff_body_lines("@@\n--- a comment\n+x", Some("f.lua"), false);
    assert_eq!(lines[1].spans[0].content.as_ref(), "-");
}

#[test]
fn uppercase_extensions_keep_syntax_highlighting_in_snippets_and_headers() {
    let hunk = "@@ -1 +1 @@\n-let old = 1;\n+let new = 2;";
    let expected = diff_body_lines(hunk, Some("src/main.rs"), false);
    assert!(expected[1].spans.len() > 4, "Rust code is tokenized");
    assert_eq!(diff_body_lines(hunk, Some("src/main.RS"), false), expected);

    for header in [
        "diff --git a/src/main.RS b/src/main.RS",
        "+++ b/src/main.RS",
    ] {
        let lines = diff_body_lines(&format!("{header}\n{hunk}"), None, false);
        assert_eq!(&lines[1..], expected.as_slice());
    }
}

#[test]
fn an_extensionless_file_is_looked_up_by_name() {
    assert_eq!(language_of("build/Makefile").as_deref(), Some("Makefile"));
    assert_eq!(language_of("a/b.test.ts").as_deref(), Some("typescript"));
    assert_eq!(language_of(".bashrc").as_deref(), Some(".bashrc"));
}
