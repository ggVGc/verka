use super::*;
use ratatui::backend::TestBackend;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;
use ratatui::Terminal;

#[test]
fn link_mode_washes_everything_but_the_entries() {
    let text = "see `src/app.rs:4` and [docs](/docs.md) or `Provider::x()`";
    let lines = markdown::markdown_block_lines_with_links(
        text,
        Style::default().fg(theme::TEXT),
        "",
        markdown::LinkDisplay::Compact,
    );
    let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new(lines.clone()), frame.area());
            tint_all_but_entries(frame, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let row: String = (0..80).map(|x| buffer[(x, 0)].symbol()).collect();
    let washed = |word: &str| {
        let start = row.find(word).unwrap() as u16;
        (start..start + word.len() as u16).all(|x| buffer[(x, 0)].fg == theme::MODAL_BACKDROP)
    };
    let bright = |word: &str| {
        let start = row.find(word).unwrap() as u16;
        (start..start + word.len() as u16).all(|x| buffer[(x, 0)].fg != theme::MODAL_BACKDROP)
    };

    assert!(bright("src/app.rs:4"), "{row}");
    assert!(bright("docs"), "{row}");
    assert!(washed("see"), "{row}");
    assert!(washed("Provider::x()"), "{row}");
}
