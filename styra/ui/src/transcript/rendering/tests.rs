use super::*;
use crate::chrome::StatusTone;
use ratatui::{backend::TestBackend, Terminal};

fn chrome() -> PanelChrome {
    PanelChrome {
        focused: true,
        workspace: None,
        worktree: None,
        into: None,
        source_branch: None,
        parent: None,
        agent: "codex".into(),
        model: "gpt".into(),
        model_reported: true,
        effort: None,
        effort_reported: false,
        status: "idle".into(),
        status_tone: StatusTone::Idle,
        elapsed: None,
        suffix: Some("transcript".into()),
        session: None,
    }
}
fn screen(
    text: &str,
    has_entries: bool,
    all_events: bool,
    uncommitted_changes: bool,
    requested_scroll: u16,
) -> (String, u16) {
    let mut terminal = Terminal::new(TestBackend::new(50, 8)).unwrap();
    let mut limit = 0;
    terminal
        .draw(|frame| {
            limit = render(
                frame,
                &TranscriptView {
                    chrome: chrome(),
                    text,
                    has_entries,
                    all_events,
                    show_minor: false,
                    uncommitted_changes,
                    requested_scroll,
                },
                frame.area(),
            );
        })
        .unwrap();
    let output = terminal
        .backend()
        .buffer()
        .content()
        .chunks(50)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (output, limit)
}
#[test]
fn uncommitted_work_is_marked_beside_the_filter() {
    let (output, _) = screen("user: hello", true, true, true, 0);
    let bottom = output.lines().last().unwrap().to_owned();
    assert!(
        bottom.find("all events") < bottom.find("uncommitted changes"),
        "{bottom}"
    );
    let (clean, _) = screen("user: hello\nagent: hi", true, true, false, 0);
    assert!(clean.contains("all events · minor hidden"));
    assert!(clean.contains("user: hello"));
    assert!(!clean.contains("uncommitted"));
}
#[test]
fn scroll_is_measured_and_clamped() {
    let text = (0..10)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (output, limit) = screen(&text, true, false, false, u16::MAX);
    assert_eq!(limit, 4);
    assert!(output.contains("line 9"));
}
#[test]
fn empty_state_has_no_scroll() {
    let (output, limit) = screen("", false, false, false, 20);
    assert_eq!(limit, 0);
    assert!(output.contains("nothing to render yet"));
}
