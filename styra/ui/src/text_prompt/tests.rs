use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn screen(text: &str, width: u16, height: u16) -> (Vec<String>, Position) {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            render(
                frame,
                &TextPrompt::new(" path ", text).bottom(" for this interaction "),
                frame.area(),
            );
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let rows = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_owned())
                .collect::<String>()
        })
        .collect();
    let cursor = terminal.get_cursor_position().unwrap();
    (rows, cursor)
}

/// The reported case: a long path was cut off at the box's right edge.
/// Every character of it has to be on screen.
#[test]
fn a_long_value_wraps_onto_as_many_rows_as_it_needs() {
    let path =
        "/home/operator/projects/a-rather-long-directory-name/with/several/nested/levels/data.csv";
    let (rows, cursor) = screen(path, 40, 20);

    let shown: String = rows
        .iter()
        .filter_map(|row| {
            let start = row.find('│')?;
            let end = row.rfind('│')?;
            (end > start).then(|| row[start + '│'.len_utf8()..end].trim_end().to_owned())
        })
        .collect();
    assert_eq!(shown, path);
    // 36 columns of content: the path takes three rows, plus borders.
    assert_eq!(height(path, 36), 5);
    // The cursor follows the end of the text onto its last row.
    let last = rows
        .iter()
        .rposition(|row| row.contains("data.csv"))
        .unwrap();
    assert_eq!(cursor.y as usize, last);
}

#[test]
fn a_short_value_keeps_the_one_row_box() {
    assert_eq!(height("notes.txt", 36), 3);
    assert_eq!(height("", 36), 3);
}

/// Filling the last column exactly moves the cursor to a row of its own
/// rather than onto the border.
#[test]
fn a_value_that_fills_the_row_gives_the_cursor_the_next_one() {
    assert_eq!(height(&"x".repeat(36), 36), 4);
    let (lines, row, column) = layout(&"x".repeat(36), 36);
    assert_eq!(lines.len(), 2);
    assert_eq!((row, column), (1, 0));
}

/// Taller than the view, it scrolls to keep the end — and the cursor — in
/// sight rather than drawing past the screen.
#[test]
fn a_value_taller_than_the_view_keeps_its_end_visible() {
    let text = format!("{}END", "x".repeat(36 * 10));
    let (rows, cursor) = screen(&text, 40, 6);
    assert!(rows.iter().any(|row| row.contains("END")), "{rows:#?}");
    assert!(cursor.y < 6);
}
