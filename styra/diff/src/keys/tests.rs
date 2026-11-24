use super::*;
use crate::state::DiffTarget;

fn diff() -> CheckoutDiff {
    let mut diff = CheckoutDiff::read(DiffTarget {
        worktree: std::env::temp_dir().join("styra-diff-keys-no-such-repository"),
        branch: None,
        base_branch: None,
        base: "HEAD".into(),
        branch_point: false,
    });
    diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                        diff --git a/q.txt b/q.txt\n@@ -1 +1 @@\n-c\n+d\n"
        .into());
    diff
}

#[test]
fn shortcut_legend_is_modal_and_preserves_selection() {
    let mut diff = diff();
    diff.line_cursor = Some(3);
    assert_eq!(press(&mut diff, KeyCode::Char('?')), Outcome::Handled);
    assert_eq!(diff.help, Some(0));
    press(&mut diff, KeyCode::Down);
    assert_eq!(diff.help, Some(1));
    press(&mut diff, KeyCode::Char('l'));
    assert!(!diff.per_file);
    assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
    assert_eq!(diff.help, None);
    assert_eq!(diff.line_cursor, Some(3));
    press(&mut diff, KeyCode::Char('?'));
    press(&mut diff, KeyCode::Char('?'));
    assert_eq!(diff.help, None);
}

#[test]
fn file_list_position_toggles_without_changing_the_selected_file() {
    let mut diff = diff();
    press(&mut diff, KeyCode::Char('l'));
    press(&mut diff, KeyCode::Char('j'));
    assert_eq!(diff.selected_file, 1);
    assert!(!diff.files_below);

    assert_eq!(press(&mut diff, KeyCode::Char('b')), Outcome::Handled);
    assert!(diff.files_below);
    assert_eq!(diff.selected_file, 1);

    press(&mut diff, KeyCode::Char('b'));
    assert!(!diff.files_below);
    assert_eq!(diff.selected_file, 1);
}

#[test]
fn file_list_visibility_toggles_without_changing_the_selected_file() {
    let mut diff = diff();
    assert_eq!(press(&mut diff, KeyCode::Char('B')), Outcome::Handled);
    assert!(!diff.file_list_hidden);
    press(&mut diff, KeyCode::Char('l'));
    press(&mut diff, KeyCode::Char('j'));
    assert_eq!(diff.selected_file, 1);

    press(&mut diff, KeyCode::Char('B'));
    assert!(diff.file_list_hidden);
    press(&mut diff, KeyCode::Char('k'));
    assert_eq!(diff.selected_file, 0);

    press(&mut diff, KeyCode::Char('B'));
    assert!(!diff.file_list_hidden);
    assert_eq!(diff.selected_file, 0);
}

/// The report page is modal like the legend: j/k pick a file on it,
/// which it opens on, the other reading keys scroll it, a report can be
/// asked for from it, and Enter opens the picked file's patch.
#[test]
fn the_report_page_picks_a_file_to_open() {
    let mut diff = diff();
    let page = |diff: &CheckoutDiff| diff.report_page.unwrap();
    press(&mut diff, KeyCode::Char('l'));
    press(&mut diff, KeyCode::Char('j'));
    press(&mut diff, KeyCode::Char('l'));
    assert_eq!(press(&mut diff, KeyCode::Char('o')), Outcome::Handled);
    assert_eq!((page(&diff).selected, page(&diff).reveal), (1, true));
    press(&mut diff, KeyCode::PageDown);
    assert_eq!((page(&diff).scroll, page(&diff).reveal), (10, false));
    press(&mut diff, KeyCode::Char('j'));
    assert_eq!((page(&diff).selected, page(&diff).reveal), (1, true));
    press(&mut diff, KeyCode::Char('k'));
    assert_eq!(page(&diff).selected, 0);
    press(&mut diff, KeyCode::Char('G'));
    assert_eq!(page(&diff).selected, 1);
    press(&mut diff, KeyCode::Char('l'));
    assert!(!diff.per_file, "the page has the keys");
    assert_eq!(
        press(&mut diff, KeyCode::Char('E')),
        Outcome::GenerateReport
    );
    assert!(diff.report_page.is_some(), "open for the report to come");
    assert_eq!(
        press(&mut diff, KeyCode::Char('e')),
        Outcome::DescribeFile("q.txt".into()),
        "the picked file alone"
    );
    assert!(
        diff.report_page.is_some(),
        "open for its description to come"
    );

    assert_eq!(page(&diff).preview, None);
    press(&mut diff, KeyCode::Char('K'));
    assert_eq!(page(&diff).preview, None, "nothing to scroll");
    press(&mut diff, KeyCode::Char('p'));
    press(&mut diff, KeyCode::Char('J'));
    press(&mut diff, KeyCode::Char('J'));
    assert_eq!(page(&diff).preview, Some(6));
    press(&mut diff, KeyCode::Char('K'));
    assert_eq!(page(&diff).preview, Some(3));
    press(&mut diff, KeyCode::Char('k'));
    assert_eq!(page(&diff).preview, Some(0), "the next file from its top");
    press(&mut diff, KeyCode::Char('j'));

    assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);
    assert_eq!(diff.report_page, None);
    assert!(diff.per_file);
    assert_eq!(diff.selected_file, 1);

    press(&mut diff, KeyCode::Char('o'));
    press(&mut diff, KeyCode::Char('g'));
    assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
    assert_eq!(diff.report_page, None);
    assert_eq!(diff.selected_file, 1, "closed without opening");

    press(&mut diff, KeyCode::Char('o'));
    assert_eq!(page(&diff).preview, Some(0), "it opens as it was left");
    press(&mut diff, KeyCode::Char('p'));
    press(&mut diff, KeyCode::Esc);
    press(&mut diff, KeyCode::Char('o'));
    assert_eq!(page(&diff).preview, None);
}

fn press(diff: &mut CheckoutDiff, code: KeyCode) -> Outcome {
    diff.handle_key(KeyEvent::from(code))
}

/// Esc clears a file filter before it closes the view, and a letter typed
/// into the filter is part of it rather than a command.
#[test]
fn esc_closes_once_a_filter_is_cleared() {
    let mut diff = diff();
    assert_eq!(press(&mut diff, KeyCode::Char('l')), Outcome::Handled);
    assert_eq!(press(&mut diff, KeyCode::Char('/')), Outcome::Handled);
    assert_eq!(press(&mut diff, KeyCode::Char('q')), Outcome::Handled);
    assert_eq!(diff.search.query(), Some("q"));
    assert_eq!(diff.selected_file, 1);
    assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);

    assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
    assert_eq!(diff.search.query(), None);
    assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Close);

    assert_eq!(press(&mut diff, KeyCode::Char('/')), Outcome::Handled);
    assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Handled);
    assert_eq!(diff.search.query(), Some("d"));
    assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);
    assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Toggle);
}

#[test]
fn view_keys_act_and_the_rest_go_to_the_host() {
    let mut diff = diff();
    for key in ['r', 'l', 'j', 'C', 't', 'e', 'w', 'L', 'm'] {
        assert_eq!(press(&mut diff, KeyCode::Char(key)), Outcome::Handled);
    }
    assert!(!diff.word_highlights);
    assert!(diff.thumbnail_hidden);
    assert!(
        matches!(diff.layers, Some(Err(_))),
        "layered, saying why there is no repository to read them from"
    );
    assert!(diff.report.hidden);
    assert!(diff.hide_removed);
    assert!(diff.per_file);
    assert!(!diff.compact_dirs);
    assert_eq!(diff.tint, crate::Tint::Half);
    assert_eq!(diff.selected_file, 1);
    assert_eq!(press(&mut diff, KeyCode::Char('v')), Outcome::OpenFile);
    assert_eq!(
        press(&mut diff, KeyCode::Char('E')),
        Outcome::GenerateReport
    );
    assert_eq!(press(&mut diff, KeyCode::Char('d')), Outcome::Toggle);
    for key in ['q', 'i', 'y'] {
        assert_eq!(press(&mut diff, KeyCode::Char(key)), Outcome::Ignored);
    }
    let control_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
    assert_eq!(diff.handle_key(control_r), Outcome::Ignored);
}

/// Esc leaves line selection before it clears a filter or closes, and
/// Enter or `V` leaves it as it entered.
#[test]
fn esc_leaves_line_selection_first() {
    let mut diff = diff();
    assert_eq!(press(&mut diff, KeyCode::Enter), Outcome::Handled);
    assert_eq!(diff.line_cursor, Some(2), "-a, the first line of a hunk");
    assert_eq!(press(&mut diff, KeyCode::Char('j')), Outcome::Handled);
    assert_eq!(diff.line_cursor, Some(3));
    assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Handled);
    assert_eq!(diff.line_cursor, None);
    assert_eq!(press(&mut diff, KeyCode::Char('V')), Outcome::Handled);
    assert_eq!(press(&mut diff, KeyCode::Char('V')), Outcome::Handled);
    assert_eq!(diff.line_cursor, None);
    assert_eq!(press(&mut diff, KeyCode::Esc), Outcome::Close);
}

/// In line selection j and k keep the cursor to added and removed
/// lines, stepping over unchanged lines, file headers and hunk cuts to
/// the next change; J and K go to the first line of the next and previous
/// block of changes, and g and G to the first and last change. The arrows
/// go over every line.
#[test]
fn line_selection_moves_through_changes_only() {
    let mut diff = diff();
    // Lines: a.txt's header 0, hunk 1, " x" 2, "-a" 3, "+b" 4, hunk 5,
    // "-c" 6, then twelve added lines 7 to 18, b.txt's header 19, hunk
    // 20, " y" 21, "+d" 22.
    diff.diff = Ok(format!(
        "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n x\n-a\n+b\n\
             @@ -9 +9,12 @@\n-c\n{}\
             diff --git a/b.txt b/b.txt\n@@ -1 +1,2 @@\n y\n+d\n",
        (0..12).map(|n| format!("+add{n}\n")).collect::<String>()
    ));
    diff.scroll.note_viewport(40);
    let mut keys = |code, expected: usize, why: &str| {
        assert_eq!(press(&mut diff, code), Outcome::Handled);
        assert_eq!(diff.line_cursor, Some(expected), "{code:?}: {why}");
    };
    keys(
        KeyCode::Enter,
        3,
        "the first change, past headers and \" x\"",
    );
    keys(KeyCode::Char('j'), 4, "+b");
    keys(KeyCode::Char('j'), 6, "over the second hunk's cut to -c");
    keys(KeyCode::Char('J'), 22, "the next block, b.txt's +d");
    keys(KeyCode::Char('J'), 22, "no block further down");
    keys(KeyCode::Char('K'), 6, "the previous block's first line");
    keys(KeyCode::Char('K'), 3, "and the one before");
    keys(KeyCode::Char('K'), 3, "no block further up");
    keys(KeyCode::Char('j'), 4, "+b");
    keys(KeyCode::Char('J'), 6, "from inside a block, the next one");
    keys(KeyCode::Char('G'), 22, "the last change, over \" y\"");
    keys(
        KeyCode::Char('k'),
        18,
        "over b.txt's header, cut and \" y\"",
    );
    keys(
        KeyCode::Char('K'),
        6,
        "from inside a block, its own first line",
    );
    keys(KeyCode::Char('g'), 3, "the first change");
    keys(KeyCode::Char('k'), 3, "nowhere further up");
    // The arrows go over every line, unchanged ones, headers and cuts too.
    keys(KeyCode::Up, 2, "\" x\"");
    keys(KeyCode::Up, 1, "the first hunk's cut");
    keys(KeyCode::Up, 0, "a.txt's header");
    keys(KeyCode::Up, 0, "nowhere further up");
    keys(KeyCode::Down, 1, "the cut again");
    keys(KeyCode::Char('j'), 3, "from the cut, the first change");
    keys(KeyCode::Char('j'), 4, "+b");
    keys(KeyCode::Down, 5, "the second hunk's cut");
    keys(KeyCode::Char('k'), 4, "back to +b");
    let control = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL);
    assert_eq!(diff.handle_key(control), Outcome::Ignored);
}

/// In line selection `p` shows the whole file the cursor is in beside the
/// diff, the cursor's line marked, and the preview follows the cursor.
/// Out of line selection it is not shown, but stays asked for.
#[cfg(feature = "renderer")]
#[test]
fn p_previews_the_cursors_file_with_its_line_marked() {
    let directory = tempfile::tempdir().unwrap();
    let text: String = (1..=40).map(|n| format!("line {n}\n")).collect();
    std::fs::write(directory.path().join("a.txt"), text).unwrap();
    let mut diff = CheckoutDiff::supplied(
        "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n\
             @@ -20 +20,2 @@\n-old\n+line 20\n+line 21\n"
            .into(),
        crate::Supplied {
            label: "test".into(),
            root: Some(directory.path().to_owned()),
        },
    );
    let shown = |diff: &CheckoutDiff| {
        diff.view(ratatui::widgets::Block::bordered())
            .file_preview
            .map(|file| (file.path.to_owned(), file.line))
    };
    assert_eq!(press(&mut diff, KeyCode::Char('p')), Outcome::Handled);
    assert!(!diff.file_preview, "line selection only");

    press(&mut diff, KeyCode::Enter);
    assert_eq!(shown(&diff), None);
    press(&mut diff, KeyCode::Char('p'));
    assert_eq!(shown(&diff), Some(("a.txt".into(), Some(20))), "-old");
    press(&mut diff, KeyCode::Char('j'));
    press(&mut diff, KeyCode::Char('j'));
    assert_eq!(shown(&diff), Some(("a.txt".into(), Some(21))), "+line 21");

    let screen = crate::test_support::rows(&crate::test_support::draw(160, 30, |frame| {
        crate::render(
            frame,
            &diff.view(ratatui::widgets::Block::bordered()),
            frame.area(),
        );
    }));
    let marked: Vec<_> = screen
        .iter()
        .filter_map(|row| row.split_once('▶').map(|(_, marked)| marked))
        .collect();
    assert_eq!(marked.len(), 1, "{screen:#?}");
    assert!(marked[0].starts_with("   21 │ line 21"), "{screen:#?}");
    let row = screen.iter().position(|row| row.contains('▶')).unwrap();
    assert!((12..=17).contains(&row), "kept in the middle: {screen:#?}");
    assert!(
        !screen.iter().any(|row| row.contains("    1 │ line 1 ")),
        "scrolled past the file's top: {screen:#?}"
    );

    press(&mut diff, KeyCode::Esc);
    assert_eq!(shown(&diff), None);
    press(&mut diff, KeyCode::Enter);
    assert_eq!(
        shown(&diff),
        Some(("a.txt".into(), Some(20))),
        "still asked for"
    );
    press(&mut diff, KeyCode::Char('p'));
    assert_eq!(shown(&diff), None);
}

/// A key answers to one command, so the table cannot shadow a row.
#[test]
fn no_key_is_bound_twice() {
    let codes: Vec<_> = BINDINGS.iter().flat_map(|binding| binding.codes).collect();
    for (index, code) in codes.iter().enumerate() {
        assert!(!codes[index + 1..].contains(code), "{code:?} bound twice");
    }
}
