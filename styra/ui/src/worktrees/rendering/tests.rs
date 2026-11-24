use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn rendered(view: &WorktreePickerView<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| render_worktree_picker(frame, view))
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

/// The list says what each row is, and the pane beneath it says where the
/// row under the cursor is on disk — the part a row has no room for.
#[test]
fn the_selected_rows_checkout_is_spelled_out_beneath_the_list() {
    let rows = vec![
        "fix-flaky-test-s-1  fix the flaky test · live".to_owned(),
        "tidy-up-s-2  s-2 · completed".to_owned(),
    ];
    let list = FuzzyList::at(&rows, 0);
    let detail = [
        ("worktree", "/state/worktrees/fix-flaky-test-s-1".to_owned()),
        ("branch", "styra/fix-flaky-test-s-1".to_owned()),
    ];
    let screen = rendered(&WorktreePickerView {
        scope: "this Workspace",
        rows: &rows,
        list: &list,
        detail: &detail,
        notice: Some("no session works in this worktree"),
    });

    assert!(screen.contains("worktrees · this Workspace"), "{screen}");
    assert!(screen.contains("fix the flaky test · live"), "{screen}");
    assert!(screen.contains("tidy-up-s-2"), "{screen}");
    assert!(
        screen.contains("/state/worktrees/fix-flaky-test-s-1"),
        "{screen}"
    );
    assert!(screen.contains("styra/fix-flaky-test-s-1"), "{screen}");
    assert!(
        screen.contains("no session works in this worktree"),
        "{screen}"
    );
}
