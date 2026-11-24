use super::*;
use ratatui::style::Color;

#[test]
fn truncation_preserves_graphemes_styles_and_column_budget() {
    let style = Style::default().fg(Color::Green);
    let line = Line::from(vec![Span::raw("a"), Span::styled("界e\u{301}👩‍💻xyz", style)])
        .style(Style::default().bg(Color::Blue));
    for columns in 0..12 {
        let clipped = truncate(line.clone(), columns, None);
        assert!(clipped.width() <= columns);
        assert_eq!(clipped.style, line.style);
    }
    assert_eq!(truncate(line.clone(), 5, None).to_string(), "a界e\u{301}…");
    let clipped = truncate(line, 7, None);
    assert_eq!(clipped.to_string(), "a界e\u{301}👩‍💻…");
    assert_eq!(clipped.spans.last().unwrap().style, style);
}

#[test]
fn middle_truncation_keeps_both_ends() {
    let style = Style::default().fg(Color::Green);
    let line = Line::from(vec![Span::raw("ab"), Span::styled("cd界fgh", style)]);
    for columns in 0..12 {
        assert!(truncate_middle(line.clone(), columns).width() <= columns);
    }
    assert_eq!(truncate_middle(line.clone(), 9).to_string(), "abcd界fgh");
    assert_eq!(truncate_middle(line.clone(), 7).to_string(), "abc…fgh");
    assert_eq!(truncate_middle(line.clone(), 6).to_string(), "abc…gh");
    // A wide grapheme that does not fit the end's share leaves its
    // column to the start.
    let wide = Line::from("abcdef界g");
    assert_eq!(truncate_middle(wide, 6).to_string(), "abcd…g");
    let cut = truncate_middle(line, 5);
    assert_eq!(cut.to_string(), "ab…gh");
    assert_eq!(cut.spans.last().unwrap().style, style);
}

#[test]
fn wrapping_respects_columns_and_keeps_unicode_and_styles() {
    let style = Style::default().fg(Color::Red);
    let line = Line::from(Span::styled("界界e\u{301}👩‍💻abcd", style))
        .style(Style::default().bg(Color::Blue));
    let rows = wrap_line(line.clone(), 4, 0);
    assert_eq!(
        rows.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["界界", "e\u{301}👩‍💻a", "bcd"]
    );
    assert!(rows
        .iter()
        .all(|row| row.width() <= 4 && row.style == line.style));
    assert!(rows
        .iter()
        .flat_map(|row| &row.spans)
        .all(|span| span.style == style));
    // Hanging indents must still leave room for wide characters.
    assert!(wrap_line(line.clone(), 3, 2)
        .iter()
        .all(|row| row.width() <= 3));
    assert!(wrap_line(line, 1, 0).iter().all(|row| row.width() <= 1));
}

#[test]
fn editable_text_keeps_spaces_newlines_and_emoji() {
    let rows = wrap_input("ab  界👩‍💻\n\ne\u{301}", 4, Style::default());
    assert_eq!(
        rows.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["ab  ", "界👩‍💻", "", "e\u{301}"]
    );
}
