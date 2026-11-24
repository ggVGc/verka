use super::*;
use ratatui::{backend::TestBackend, Terminal};

fn screen(width: u16, view: &FooterView<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, 1)).unwrap();
    terminal
        .draw(|frame| render(frame, view, None, frame.area()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}
#[test]
fn shows_context_and_standing_state() {
    let quota = vec![Segment {
        text: " codex: 80%".into(),
        tone: Tone::Warning,
        bold: true,
    }];
    let output = screen(
        120 + BUILD_BRANCH.width() as u16,
        &FooterView {
            working_directory: "/workspace",
            quota: &quota,
            auto_retry: true,
            auto_commit: true,
            background: Some("describing edits"),
        },
    );
    for expected in [
        "codex: 80%",
        "rate-limit retry: on",
        "^G auto-commit",
        "⟳ describing edits",
        "/workspace",
    ] {
        assert!(output.contains(expected), "missing {expected}: {output}");
    }
    if BUILD_BRANCH == "main" {
        assert!(!output.contains("[build:"));
    } else {
        assert!(output.contains(&format!("[build: {BUILD_BRANCH}]")));
    }
}
