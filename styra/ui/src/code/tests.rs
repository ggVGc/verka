use super::*;

/// The colors of one line's content, past the gutter.
fn colors(line: &Line<'_>) -> Vec<Option<Color>> {
    line.spans[1..].iter().map(|span| span.style.fg).collect()
}

/// One line's first content span, past the gutter.
fn content<'a>(line: &'a Line<'_>) -> &'a Span<'a> {
    &line.spans[1]
}

#[test]
fn plain_and_highlighted_blocks_carry_the_gutter_on_every_row() {
    for (source, language) in [("one\ntwo", None), ("fn main() {}", Some("rust"))] {
        let lines = code_block_lines(source, language, theme::TEXT, false, "  ");
        assert!(!lines.is_empty(), "{language:?}");
        for line in &lines {
            assert_eq!(line.spans[0].content.as_ref(), "  │ ", "{language:?}");
            assert_eq!(
                line.spans[0].style.fg,
                Some(theme::INACTIVE),
                "{language:?}"
            );
        }
    }
}

#[test]
fn bash_blocks_are_tokenized() {
    let lines = code_block_lines(
        "grep -n 'needle' $FILE",
        Some("bash"),
        theme::TEXT,
        false,
        "  ",
    );

    let colors = colors(&lines[0]);
    assert!(colors.len() > 2, "{colors:?}"); // tokenized, not one plain span
}

#[test]
fn diff_lines_are_colored_by_their_marker() {
    let lines = code_block_lines(
        "@@ -1 +1 @@\n-gone\n+added\n--- a/f\n+++ b/f",
        None,
        theme::TEXT,
        false,
        "",
    );

    let fg: Vec<Option<Color>> = lines.iter().map(|line| content(line).style.fg).collect();
    assert_eq!(
        fg,
        vec![
            Some(theme::ACCENT),
            Some(theme::ERROR),
            Some(theme::SUCCESS),
            // File headers are not additions or removals.
            Some(theme::TEXT),
            Some(theme::TEXT),
        ]
    );
}

#[test]
fn a_suspicious_success_colors_its_diagnostic_even_in_a_bash_block() {
    let lines = code_block_lines(
        "ls missing\nls: no such file or directory",
        Some("bash"),
        theme::TEXT,
        true,
        "",
    );

    assert_eq!(content(&lines[1]).style.fg, Some(theme::ERROR));
}

#[test]
fn tabs_become_spaces_so_columns_do_not_collapse() {
    let lines = code_block_lines("a\tb", None, theme::TEXT, false, "");

    assert_eq!(content(&lines[0]).content.as_ref(), "a    b");
}
