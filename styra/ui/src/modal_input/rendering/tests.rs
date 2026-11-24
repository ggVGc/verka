use super::*;

fn input<'a>(parts: &'a [String], preceding: Vec<String>) -> ModalInput<'a> {
    ModalInput {
        title: " message ".into(),
        note: None,
        model: None,
        model_reported: false,
        effort: None,
        effort_reported: false,
        preceding,
        notice: None,
        parts,
        focused: parts.len() - 1,
        choosing: false,
        source: None,
        placeholder: "type a message, Enter to send",
        cursor: true,
    }
}

#[test]
fn wraps_input_and_keeps_the_cursor_on_the_final_visual_row() {
    let wrapped = display(&input(&["abcdefghijk".into()], Vec::new()), 5);
    assert_eq!(wrapped.lines.len(), 3);
    assert_eq!((wrapped.cursor_col, wrapped.cursor_row), (1, 2));

    let display = display(&input(&["abcde".into()], Vec::new()), 5);
    assert_eq!(display.lines.len(), 2);
    assert_eq!((display.cursor_col, display.cursor_row), (0, 1));
}

#[test]
fn queued_lines_are_visually_secondary() {
    let display = display(
        &input(&["draft".into()], vec!["queued: send later".into()]),
        40,
    );
    let queued = display.lines[0]
        .spans
        .iter()
        .find(|span| span.content.contains("queued:"))
        .unwrap();
    assert_eq!(queued.style.fg, Some(theme::ADDITIONAL_INFO));
}

/// Several boxes each sit under a rule, and the cursor stays in the
/// current one even when a later box follows it.
#[test]
fn several_boxes_are_ruled_and_the_cursor_stays_in_the_current_one() {
    let parts = ["first".to_owned(), "second".to_owned(), String::new()];
    let mut boxes = input(&parts, Vec::new());
    boxes.focused = 1;
    let display = display(&boxes, 20);
    let rows: Vec<String> = display.lines.iter().map(|line| line.to_string()).collect();
    assert_eq!(rows.len(), 6);
    assert!(rows[0].starts_with("── 1 "));
    assert_eq!(rows[3], "second");
    assert_eq!(rows[5], "empty");
    assert_eq!((display.cursor_col, display.cursor_row), (6, 3));
}

#[test]
fn long_boxes_expand_only_while_being_edited() {
    let parts = ["one\ntwo\nthree\nfour\nfive".into(), "draft".into()];
    let mut boxes = input(&parts, Vec::new());
    let collapsed = display(&boxes, 20);
    let rows: Vec<String> = collapsed
        .lines
        .iter()
        .map(|line| line.to_string())
        .collect();
    assert_eq!(&rows[1..4], ["one", "two", "…"]);
    assert_eq!(rows[5], "draft");
    assert_eq!((collapsed.cursor_col, collapsed.cursor_row), (5, 5));

    boxes.focused = 0;
    let expanded = display(&boxes, 20);
    let rows: Vec<String> = expanded.lines.iter().map(|line| line.to_string()).collect();
    assert_eq!(&rows[1..6], ["one", "two", "three", "four", "five"]);
    assert_eq!((expanded.cursor_col, expanded.cursor_row), (4, 5));

    boxes.choosing = true;
    let choosing = display(&boxes, 20);
    assert_eq!(choosing.lines.len(), 6);
    assert_eq!(choosing.lines[3].to_string(), "…");
    assert_eq!(choosing.cursor_row, 3);
}

/// The source is noted under the message, and the cursor stays in it.
#[test]
fn the_source_is_noted_under_the_message() {
    let parts = ["look here".to_owned()];
    let mut boxes = input(&parts, Vec::new());
    boxes.source = Some("src/main.rs:12".into());
    let display = display(&boxes, 40);
    let rows: Vec<String> = display.lines.iter().map(|line| line.to_string()).collect();
    assert_eq!(rows, ["look here", "Source: src/main.rs:12"]);
    assert_eq!((display.cursor_col, display.cursor_row), (9, 0));
    assert_eq!(height(&boxes, 40), 4);
}

#[test]
fn collapsed_height_counts_wrapped_lines_and_does_not_change_text() {
    let parts = ["abcdefghijklmnopqrst".into(), "z".into()];
    let boxes = input(&parts, Vec::new());
    let collapsed = display(&boxes, 5);
    assert_eq!(collapsed.lines.len(), 6);
    assert_eq!(collapsed.lines[1].to_string(), "abcde");
    assert_eq!(collapsed.lines[2].to_string(), "fghij");
    assert_eq!(collapsed.lines[3].to_string(), "…");
    assert_eq!(parts[0], "abcdefghijklmnopqrst");
}
