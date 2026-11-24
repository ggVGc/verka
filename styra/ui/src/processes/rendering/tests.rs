use super::*;
use crate::chrome::StatusTone;
use ratatui::{backend::TestBackend, Terminal};

fn process(pid: u32, sandbox_pid: u32, parent_pid: u32, command: &str) -> SandboxProcess {
    SandboxProcess {
        pid,
        sandbox_pid,
        parent_pid,
        state: "S".into(),
        command: command.into(),
        threads: 1,
        rss_kib: 2048,
    }
}

fn chrome() -> PanelChrome {
    PanelChrome {
        focused: true,
        workspace: None,
        worktree: None,
        into: None,
        source_branch: None,
        parent: None,
        agent: "claude".into(),
        model: "opus".into(),
        model_reported: true,
        effort: None,
        effort_reported: false,
        status: "running".into(),
        status_tone: StatusTone::Idle,
        elapsed: None,
        suffix: None,
        session: None,
    }
}

fn screen(view: ProcessesView<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 10)).unwrap();
    terminal
        .draw(|frame| render(frame, &view, frame.area()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(100)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_tree_puts_children_under_their_parent_in_sandbox_pid_order() {
    let processes = [
        process(104, 4, 102, "cargo test"),
        process(101, 1, 100, "bwrap"),
        process(103, 3, 101, "sleep infinity"),
        process(102, 2, 101, "claude"),
    ];
    let rows: Vec<_> = tree(&processes)
        .iter()
        .map(|row| (row.process.sandbox_pid, row.depth))
        .collect();
    assert_eq!(rows, [(1, 0), (2, 1), (4, 2), (3, 1)]);
}

#[test]
fn processes_render_as_an_indented_tree_with_both_pids() {
    let processes = [
        process(101, 1, 100, "bwrap"),
        process(102, 2, 101, "claude --print"),
    ];
    let output = screen(ProcessesView {
        chrome: chrome(),
        listing: ProcessListing::Running(&processes),
        selected: Some(1),
    });
    assert!(output.contains("processes"));
    assert!(output.contains("COMMAND"));
    assert!(output.contains("2.0M"));
    let claude = output
        .lines()
        .find(|line| line.contains("claude --print"))
        .unwrap();
    assert!(claude.contains(" 2 "));
    assert!(claude.contains("102"));
    assert!(claude.contains("└ claude --print"));
    assert!(output.contains("2 processes · 4.0M resident · j/k: select"));
}

#[test]
fn the_pane_below_the_event_list_selects_nothing_and_offers_full_screen() {
    let processes = [process(101, 1, 100, "bwrap")];
    let output = screen(ProcessesView {
        chrome: chrome(),
        listing: ProcessListing::Running(&processes),
        selected: None,
    });
    assert!(output.contains("H: full screen"));
    assert!(!output.contains("j/k: select"));
}

#[test]
fn a_stopped_interaction_says_no_sandbox_runs() {
    let output = screen(ProcessesView {
        chrome: chrome(),
        listing: ProcessListing::NotRunning,
        selected: None,
    });
    assert!(output.contains("no sandbox is running for this interaction"));
}
