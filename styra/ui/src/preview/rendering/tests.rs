use super::*;

fn roots() -> Vec<String> {
    vec!["/tmp/styra/workspace".into(), "/home/op/project/".into()]
}

#[test]
fn unicode_preview_scroll_counts_each_wrapped_row_once() {
    let lines = wrap_preview_lines(
        vec![Line::from("界界界界界界界"), Line::from("e\u{301}👩‍💻界")],
        6,
        2,
        0,
    );
    assert!(lines.iter().all(|line| line.width() <= 6));
    assert_eq!(lines.len(), 4);
    assert_eq!(preview_scroll_limit(&lines, 6, 2), 2);
}

#[test]
fn a_path_under_the_workspace_starts_with_a_muted_shorthand() {
    for path in [
        "/tmp/styra/workspace/src/retry.rs",
        "/home/op/project/src/retry.rs",
    ] {
        let line = changed_path_line(path, &roots());
        let texts: Vec<&str> = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(texts, [WORKSPACE_SHORTHAND, "src/retry.rs"]);
        assert_eq!(line.spans[0].style.fg, Some(theme::MUTED_TEXT));
        assert_eq!(line.spans[1].style.fg, Some(message_text_color("files")));
    }
}

/// A sibling that only shares the root's leading characters is not in it,
/// and a path elsewhere is shown whole.
#[test]
fn a_path_outside_the_workspace_is_shown_whole() {
    for path in ["/tmp/styra/workspace2/a.rs", "/etc/hosts", "src/a.rs"] {
        let line = changed_path_line(path, &roots());
        assert_eq!(line.spans.len(), 1, "{path}");
        assert_eq!(line.spans[0].content, path.to_owned());
    }
}
