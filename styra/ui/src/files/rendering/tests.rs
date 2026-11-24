use super::*;
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::{Constraint, Layout},
    Terminal,
};

fn draw_content(
    height: u16,
    title: &str,
    content: &str,
    target: Option<u32>,
    scroll: u16,
) -> (Buffer, crate::preview::PreviewFeedback) {
    let mut terminal = Terminal::new(TestBackend::new(40, height)).unwrap();
    let mut feedback = None;
    terminal
        .draw(|frame| {
            feedback = Some(render_content(
                frame,
                Some(title),
                FileContent::Ready(content),
                target,
                scroll,
                frame.area(),
            ));
        })
        .unwrap();
    (terminal.backend().buffer().clone(), feedback.unwrap())
}

fn screen(files: &[FileView], content: FileContent<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| {
            let panes =
                Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(frame.area());
            render_tree(frame, files, 0, "files · focused", panes[0]);
            render_content(
                frame,
                files.first().map(|file| file.reported.as_str()),
                content,
                None,
                0,
                panes[1],
            );
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}
#[test]
fn tree_renders_prepared_content_without_io() {
    let files = vec![FileView {
        reported: "src/main.rs".into(),
        root: "/workspace".into(),
        relative: "src/main.rs".into(),
        components: vec!["src".into(), "main.rs".into()],
    }];
    let output = screen(&files, FileContent::Ready("fn main() {}\n"));
    for expected in ["/workspace", "▾ src/", "› main.rs", "fn main() {}"] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
}
#[test]
fn content_states_are_explicit() {
    assert!(screen(&[], FileContent::None).contains("no file selected"));
    assert!(screen(&[], FileContent::Empty).contains("(empty file)"));
    assert!(screen(&[], FileContent::Failed("denied")).contains("could not read file: denied"));
}

#[test]
fn a_target_line_is_numbered_and_marked() {
    let (buffer, _) = draw_content(8, "src/main.rs:2", "first\nsecond\nthird", Some(2), 0);
    let output = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(output.contains("▶    2 │ second"), "{output}");
}

#[test]
fn file_extensions_select_the_syntax_grammar_even_with_a_location_suffix() {
    assert_eq!(
        file_language(Some("src/main.rs:12")).as_deref(),
        Some("rust")
    );
    assert_eq!(
        file_language(Some("web/app.tsx:8:3")).as_deref(),
        Some("typescript")
    );
    assert_eq!(file_language(Some("lib/styra.ex:4")).as_deref(), Some("ex"));
    assert_eq!(file_language(Some("lib/styra.EX:4")).as_deref(), Some("ex"));
    assert_eq!(
        file_language(Some("src/main.RS:12")).as_deref(),
        Some("rust")
    );
    assert_eq!(file_language(Some("Makefile")).as_deref(), Some("Makefile"));
}

#[test]
fn content_scrolls_as_asked_up_to_its_last_page() {
    let content = (1..=30).map(|n| format!("line {n}\n")).collect::<String>();
    let draw = |scroll| {
        let (buffer, feedback) = draw_content(12, "notes.txt", &content, None, scroll);
        let rows: Vec<String> = buffer
            .content()
            .chunks(40)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        (feedback, rows)
    };

    let (feedback, rows) = draw(5);
    assert_eq!(feedback.effective_scroll, 5);
    assert_eq!(feedback.viewport, 10);
    assert_eq!(feedback.limit, 20);
    assert!(rows[1].contains("line 6"), "{rows:?}");

    let (feedback, rows) = draw(u16::MAX);
    assert_eq!(feedback.effective_scroll, 20);
    assert!(rows[10].contains("line 30"), "{rows:?}");
}

#[test]
fn file_types_without_a_name_here_are_still_highlighted() {
    let content = "defmodule Styra do\n  def hello, do: :world\nend\n";
    let (buffer, _) = draw_content(6, "lib/styra.ex", content, None, 0);
    let colors: BTreeSet<_> = buffer
        .content()
        .iter()
        .filter(|cell| !cell.symbol().trim().is_empty() && !"│─┌┐└┘".contains(cell.symbol()))
        .map(|cell| format!("{:?}", cell.fg))
        .collect();
    assert!(colors.len() > 2, "{colors:?}");
}
