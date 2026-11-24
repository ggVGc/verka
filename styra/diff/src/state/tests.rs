use super::*;
#[cfg(feature = "renderer")]
use crate::test_support::{draw as draw_frame, rows as buffer_rows};
use crate::test_support::{git, repository};
use crate::view::removed_rows;

fn two_files() -> CheckoutDiff {
    let mut diff = CheckoutDiff::new(
        Source::Checkout(DiffTarget {
            worktree: PathBuf::from("/w"),
            branch: None,
            base_branch: None,
            base: "4bf5c35d".into(),
            branch_point: true,
        }),
        Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
                diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n"
            .into()),
        Vec::new(),
    );
    diff.scroll.note_limit(100);
    diff
}

#[cfg(feature = "renderer")]
#[test]
fn involved_filter_pins_commits_and_keeps_navigation_and_counts_in_sync() {
    use crate::layers::Layer;
    let a = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+A\n";
    let moved = "diff --git a/old.txt b/b.txt\nsimilarity index 100%\nrename from old.txt\nrename to b.txt\n";
    let b = "diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-b\n+B\n";
    let c = "diff --git a/c.txt b/c.txt\n@@ -1 +1 @@\n-c\n+C\n";
    let mut diff = two_files();
    diff.diff = Ok(format!("{a}{moved}{c}"));
    diff.layers = Some(Ok(Layers::from_steps(&[
        (
            Layer {
                name: "first".into(),
                ..Layer::default()
            },
            format!("{a}{moved}"),
        ),
        (
            Layer {
                name: "second".into(),
                ..Layer::default()
            },
            format!("{b}{c}"),
        ),
    ])));
    diff.per_file = true;
    let names = |diff: &CheckoutDiff| {
        diff.visible_files()
            .iter()
            .map(|(_, file)| file.path.to_owned())
            .collect::<Vec<_>>()
    };
    let drawn = |diff: &CheckoutDiff, files_below| {
        let buffer = draw_frame(120, 30, |frame| {
            let mut view = diff.view(Block::default());
            view.files_below = files_below;
            crate::view::render(frame, &view, frame.area());
        });
        buffer_rows(&buffer).join("\n")
    };
    for below in [false, true] {
        let shown = drawn(&diff, below);
        assert!(shown.contains("2/3 files involved · c: filter"), "{shown}");
    }
    diff.handle_key(KeyCode::Char('c').into());
    assert_eq!(diff.involved_layers, Some(vec![0]));
    assert_eq!(names(&diff), ["a.txt", "b.txt"]);
    for below in [false, true] {
        let shown = drawn(&diff, below);
        assert!(
            shown.contains("2/3 files involved · c: show all"),
            "{shown}"
        );
        assert!(!shown.contains("c.txt"), "{shown}");
    }
    diff.apply(Action::Down);
    assert_eq!(diff.selected_file, 1);
    assert!(diff.shown_text().unwrap().contains("rename to b.txt"));
    // b also belongs to the second commit, but browsing it keeps the
    // original set pinned rather than pulling c into the list.
    assert_eq!(names(&diff), ["a.txt", "b.txt"]);
    diff.apply(Action::Down);
    assert_eq!(diff.selected_file, 1);
    diff.apply(Action::Search);
    diff.search_key(KeyCode::Char('b').into());
    diff.search_key(KeyCode::Enter.into());
    assert_eq!(names(&diff), ["b.txt"]);
    diff.handle_key(KeyCode::Char('c').into());
    assert!(diff.involved_layers.is_none());
    assert_eq!(names(&diff), ["b.txt"]);
    assert!(diff.clear_filter());
    assert_eq!(names(&diff), ["a.txt", "b.txt", "c.txt"]);
    diff.selected_file = 0;
    diff.line_cursor = Some(3);
    diff.handle_key(KeyCode::Char('c').into());
    assert_eq!(diff.involved_layers, Some(vec![0]));
    diff.handle_key(KeyCode::Esc.into());
    assert!(diff.line_cursor.is_none());
    diff.handle_key(KeyCode::Esc.into());
    assert!(diff.involved_layers.is_none());
    assert_eq!(names(&diff).len(), 3);
    diff.layers = None;
    diff.handle_key(KeyCode::Char('c').into());
    assert!(diff.involved_layers.is_none());
}

#[cfg(feature = "renderer")]
#[test]
fn prompts_mark_files_and_lines_and_preview_in_both_layouts() {
    let mut diff = two_files();
    for per_file in [false, true] {
        for split in [false, true] {
            for hidden in [false, true] {
                diff.per_file = per_file;
                diff.selected_file = 1;
                diff.line_cursor = Some(if per_file { 3 } else { 7 });
                diff.side_by_side = split;
                diff.hide_removed = hidden;
                let buffer = draw_frame(120, 35, |frame| {
                    let mut view = diff.view(Block::default());
                    view.prompts =
                        vec![
                            crate::Prompt::from_message("please fix this\n\nSource: b.txt:1", 2)
                                .unwrap(),
                        ];
                    crate::render(frame, &view, frame.area());
                });
                let rows: Vec<String> = buffer_rows(&buffer);
                assert!(rows.iter().any(|r| r.contains("please fix this")));
                assert!(rows.iter().any(|r| r.contains("Enter: interaction log")));
                assert!(
                    rows.iter().any(|r| r.contains("b.txt") && r.contains('●')),
                    "file marker: {rows:?}"
                );
                assert!(
                    rows.iter().any(|r| r.contains('●') && r.contains('d')),
                    "line marker: {rows:?}"
                );
            }
        }
    }
}

#[cfg(feature = "renderer")]
#[test]
fn layout_changes_and_resizing_preserve_source_locations() {
    let mut diff = two_files();
    diff.diff = Ok(format!(
        "diff --git a/a.txt b/a.txt\n@@ -1,60 +1,60 @@\n{}",
        (0..30)
            .map(|i| format!("-old{i}\n+new{i}\n keep{i}\n"))
            .collect::<String>()
    ));
    let draw = |diff: &mut CheckoutDiff, width| {
        let mut feedback = CheckoutDiffFeedback::default();
        draw_frame(width, 16, |frame| {
            feedback = crate::render(frame, &diff.view(Block::default()), frame.area());
        });
        diff.note(feedback);
    };
    let location = |diff: &CheckoutDiff| {
        let text = diff.shown_text().unwrap();
        let plan =
            crate::split::source_rows(text, diff.hide_removed, diff.rendered_side, diff.per_file);
        position(
            text,
            line_at(&plan, usize::from(diff.scroll.clamped())).unwrap(),
        )
        .unwrap()
        .1
    };
    draw(&mut diff, 120);
    diff.scroll.offset = 13;
    let before = location(&diff);
    diff.apply(Action::ToggleLayout);
    draw(&mut diff, 120);
    assert!(diff.rendered_side);
    assert_eq!(location(&diff), before);
    draw(&mut diff, 60);
    assert!(!diff.rendered_side);
    assert!(diff.side_by_side);
    assert_eq!(location(&diff), before);
    draw(&mut diff, 120);
    assert!(diff.rendered_side);
    assert_eq!(location(&diff), before);
    diff.apply(Action::ToggleRemoved);
    draw(&mut diff, 120);
    assert!(!diff.rendered_side);
    assert_eq!(location(&diff), before);
    diff.apply(Action::ToggleRemoved);
    draw(&mut diff, 120);
    assert!(diff.rendered_side);
    assert_eq!(location(&diff), before);
    diff.jump_to(Path::new("/w/a.txt"), Some(41)).unwrap();
    draw(&mut diff, 160);
    assert_eq!(location(&diff), Some(41));
}

#[test]
fn jump_focuses_the_matching_file_and_hunk_line() {
    let mut diff = two_files();
    diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n\
            diff --git a/b.txt b/b.txt\n@@ -10,3 +10,3 @@\n keep\n-old\n+new\n after\n\
            @@ -30 +30 @@\n-last\n+end\n"
        .into());
    diff.scroll.offset = 2;
    diff.jump_to(Path::new("/w/b.txt"), Some(30)).unwrap();
    assert!(diff.per_file);
    assert_eq!(diff.selected_file, 1);
    assert_eq!(diff.combined_scroll.offset, 2);
    let shown = rows(diff.shown_text().unwrap(), false, true);
    let row = line_at(&shown, usize::from(diff.scroll.clamped())).unwrap();
    assert_eq!(diff.shown_text().unwrap().lines().nth(row), Some("+end"));

    diff.hide_removed = true;
    diff.jump_to(Path::new("/w/b.txt"), Some(11)).unwrap();
    let shown = rows(diff.shown_text().unwrap(), true, true);
    let row = line_at(&shown, usize::from(diff.scroll.clamped())).unwrap();
    assert_eq!(diff.shown_text().unwrap().lines().nth(row), Some("+new"));
    diff.jump_to(Path::new("/w/b.txt"), Some(10)).unwrap();
    let offset = diff.scroll.offset;
    for (path, line) in [
        ("/w/b.txt", Some(20)),
        ("/w/unchanged", None),
        ("/else/b.txt", None),
    ] {
        assert!(diff.jump_to(Path::new(path), line).is_err());
        assert_eq!(diff.scroll.offset, offset);
        assert_eq!(diff.selected_file, 1);
    }
    diff.jump_to(Path::new("/w/a.txt"), None).unwrap();
    assert_eq!((diff.selected_file, diff.scroll.offset), (0, 0));
}

/// A file appearing ahead of the selected one moves its index, not the
/// selection: the refreshed view stays on the file being read.
#[test]
fn a_refresh_picks_up_new_work_and_keeps_the_selected_file() {
    let (_directory, root) = repository("styra-checkout-diff-refresh");
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    std::fs::write(root.join("b.txt"), "one\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    let start = git(&root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("b.txt"), "two\n").unwrap();
    let mut diff = CheckoutDiff::read(DiffTarget {
        worktree: root.clone(),
        branch: None,
        base_branch: None,
        base: start,
        branch_point: true,
    });
    diff.scroll.offset = 3;
    let added = diff
        .diff
        .as_ref()
        .unwrap()
        .lines()
        .position(|line| line == "+two");
    diff.line_cursor = added;

    assert!(!diff.refresh());
    git(&root, &["tag", "baseline"]);
    assert!(!diff.refresh());
    assert!(diff.base_names.iter().any(|name| name == "tag: baseline"));
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    assert!(diff.refresh());

    assert!(diff.diff.as_ref().unwrap().contains("a/a.txt"));
    assert_eq!(diff.selected_path().as_deref(), Some("b.txt"));
    assert_eq!(diff.scroll.offset, 3);
    assert_ne!(diff.line_cursor, added, "a.txt's patch now comes first");
    assert_eq!(
        diff.source(),
        Some(("b.txt", Some(1))),
        "the cursor stays on b.txt"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A supplied diff is only ever changed by the host: refreshing reads
/// nothing, and a replacement keeps the selected file as a refresh does.
/// Its files open from its root, and not at all without one.
#[test]
fn a_supplied_diff_changes_only_when_replaced() {
    let one = "diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n";
    let mut diff = CheckoutDiff::supplied(
        one.into(),
        Supplied {
            label: "turn 1".into(),
            root: Some(PathBuf::from("/w")),
        },
    );
    assert!(!diff.refresh());
    assert_eq!(diff.root(), Some(Path::new("/w")));
    diff.apply(Action::ToggleFiles);
    diff.scroll.offset = 3;

    let two = format!("diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-a\n+b\n{one}");
    assert!(diff.replace(Ok(two.clone())));
    assert!(!diff.replace(Ok(two)));
    assert_eq!(diff.selected_path().as_deref(), Some("b.txt"));
    assert_eq!(diff.scroll.offset, 3);
    assert!(diff.jump_to(Path::new("/w/a.txt"), None).is_ok());

    let Source::Supplied(supplied) = &mut diff.source else {
        unreachable!("the diff was supplied");
    };
    supplied.root = None;
    assert!(diff.location().is_err());
    assert!(diff.jump_to(Path::new("/w/a.txt"), None).is_err());
}

/// The same keys scroll the combined diff and walk the files of the
/// per-file one, g/G to its first and last; the file-only actions do
/// nothing in the combined diff.
#[test]
fn up_and_down_move_through_lines_or_files_by_view() {
    let mut diff = two_files();
    diff.apply(Action::Down);
    assert_eq!((diff.scroll.offset, diff.selected_file), (1, 0));
    diff.apply(Action::FilesDown);
    diff.apply(Action::Search);
    assert_eq!((diff.scroll.offset, diff.selected_file), (1, 0));
    assert!(!diff.search.typing());

    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.scroll.offset, 0);
    diff.apply(Action::Down);
    assert_eq!((diff.scroll.offset, diff.selected_file), (0, 1));
    diff.apply(Action::Up);
    assert_eq!(diff.selected_file, 0);
    diff.apply(Action::Last);
    assert_eq!((diff.scroll.offset, diff.selected_file), (0, 1));
    diff.apply(Action::First);
    assert_eq!(diff.selected_file, 0);
}

/// The file to open is the one the top row is in, at the new file's line
/// there, counted the same with removed lines hidden; the per-file view
/// opens its selected file; a deleted file is not opened.
#[test]
fn the_location_is_the_file_and_line_at_the_top() {
    let root = std::env::temp_dir().join("styra-diff-location");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.rs"), "").unwrap();
    let mut diff = two_files();
    let Source::Checkout(target) = &mut diff.source else {
        unreachable!("two_files reads a checkout");
    };
    target.worktree = root.clone();
    // Rows: 0 a.rs's header, 1 its rule, 2 its hunk, 3 " keep" (10),
    // 4 "-old", 5 "+new" (11), 6 " after" (12), 7 the rule above
    // gone.rs's header, 8 that header. Git path markers are hidden.
    let text = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
                    @@ -10,3 +10,3 @@\n keep\n-old\n+new\n after\n\
                    diff --git a/gone.rs b/gone.rs\n--- a/gone.rs\n+++ /dev/null\n\
                    @@ -1 +0,0 @@\n-gone\n";
    assert_eq!(
        removed_rows(text).len(),
        text.lines().count(),
        "removal flags retain raw source indices"
    );
    diff.diff = Ok(text.into());
    let a = root.join("a.rs");

    assert_eq!(diff.location(), Ok((a.clone(), None)));
    diff.scroll.offset = 1;
    assert_eq!(
        diff.location(),
        Ok((a.clone(), Some(10))),
        "the rule anchors to the first visible hunk"
    );
    diff.scroll.offset = 3;
    assert_eq!(diff.location(), Ok((a.clone(), Some(10))));
    diff.scroll.offset = 4;
    assert_eq!(diff.location(), Ok((a.clone(), Some(11))), "where -old was");
    diff.hide_removed = true;
    assert_eq!(
        diff.location(),
        Ok((a.clone(), Some(11))),
        "+new, row 4 of those shown"
    );
    diff.hide_removed = false;
    for top in [7, 8] {
        diff.scroll.offset = top;
        assert_eq!(
            diff.location(),
            Err("gone.rs is deleted in the working tree".into()),
            "the rule above a header belongs to the file below it"
        );
    }

    diff.apply(Action::ToggleFiles);
    diff.apply(Action::Down);
    assert_eq!(diff.selected_file, 1);
    assert!(
        diff.location().is_err(),
        "the per-file view opens its selected file"
    );
    diff.apply(Action::Up);
    assert_eq!(diff.location(), Ok((a, Some(10))));
    let _ = std::fs::remove_dir_all(&root);
}

/// Hiding and showing removed lines keeps the row at the top in place. A
/// removed row that goes is replaced there by the next row that stays,
/// and a rule at the top stays there.
#[test]
fn toggling_removed_lines_keeps_the_top_row() {
    let mut diff = two_files();
    // Rows: header, its rule, hunk, " a", "-b", "-c", "+B", " d".
    diff.diff = Ok("diff --git a/f b/f\n@@ -1,4 +1,3 @@\n a\n-b\n-c\n+B\n d\n".into());
    let mut toggled = |top: u16| {
        diff.scroll.offset = top;
        diff.apply(Action::ToggleRemoved);
        let hidden = diff.scroll.offset;
        diff.apply(Action::ToggleRemoved);
        (hidden, diff.scroll.offset)
    };
    assert_eq!(toggled(3), (3, 3), "\" a\" stays where it was");
    assert_eq!(toggled(4), (4, 6), "\"-b\" goes; \"+B\" takes the top");
    assert_eq!(toggled(5), (4, 6));
    assert_eq!(toggled(7), (5, 7));
    assert_eq!(toggled(1), (1, 1), "the header's rule");
    assert_eq!(toggled(0), (0, 0));
}

/// Going back to the combined diff from the per-file view returns to the
/// row it was left at, hidden removed lines and all.
#[test]
fn the_combined_diff_keeps_its_place_across_the_per_file_view() {
    let mut diff = two_files();
    // Rows: a.txt's 0-4, the rule above b.txt's header 5, the header 6,
    // its rule 7, hunk 8, "-c" 9, "+d" 10.
    diff.scroll.offset = 8;
    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.scroll.offset, 0, "a file's patch opens at its top");
    diff.apply(Action::Down);
    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.scroll.offset, 8);

    diff.scroll.offset = 10;
    diff.apply(Action::ToggleFiles);
    diff.apply(Action::ToggleRemoved);
    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.scroll.offset, 8, "\"+d\" is row 8 of those shown");
    diff.apply(Action::ToggleFiles);
    diff.apply(Action::ToggleRemoved);
    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.scroll.offset, 10);
}

/// Each file of the per-file view opens where it was left, by line, so
/// hiding removed lines in between keeps it on the same one.
#[test]
fn each_file_keeps_its_place_in_the_per_file_view() {
    let mut diff = CheckoutDiff::supplied(
        "diff --git a/a.txt b/a.txt\n@@ -1,3 +1,3 @@\n-a\n-b\n-c\n+d\n+e\n+f\n\
             diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-c\n+d\n"
            .into(),
        Supplied {
            label: String::new(),
            root: None,
        },
    );
    diff.apply(Action::ToggleFiles);
    diff.scroll.note_limit(100);
    let rows = crate::split::source_rows(diff.shown_text().unwrap(), false, false, true);
    let plus_e = rows
            .iter()
            .position(|row| matches!(row, Row::Line(index) if diff.shown_text().unwrap().lines().nth(*index) == Some("+e")))
            .unwrap();
    diff.scroll.offset = u16::try_from(plus_e).unwrap();
    diff.apply(Action::Down);
    assert_eq!(
        (diff.selected_file, diff.scroll.offset),
        (1, 0),
        "a new file opens at its top"
    );
    diff.scroll.offset = 1;
    diff.apply(Action::Up);
    assert_eq!(diff.scroll.offset, u16::try_from(plus_e).unwrap());
    diff.apply(Action::Down);
    assert_eq!(diff.scroll.offset, 1);

    diff.apply(Action::ToggleRemoved);
    diff.apply(Action::Up);
    let text = diff.shown_text().unwrap();
    let rows = crate::split::source_rows(text, true, false, true);
    let top = line_at(&rows, usize::from(diff.scroll.offset)).unwrap();
    assert_eq!(text.lines().nth(top), Some("+e"));

    diff.apply(Action::ToggleFiles);
    diff.apply(Action::ToggleFiles);
    assert_eq!(
        text_at_top(&diff),
        Some("+e"),
        "and across the combined diff"
    );
    diff.open_file(1);
    assert_eq!(diff.scroll.offset, 1);
}

fn text_at_top(diff: &CheckoutDiff) -> Option<&str> {
    let text = diff.shown_text()?;
    let rows = crate::split::source_rows(text, diff.hide_removed, diff.rendered_side, true);
    text.lines()
        .nth(line_at(&rows, usize::from(diff.scroll.offset))?)
}

/// The line cursor starts on the line at the top, moves a drawn line at
/// a time — over rules, and over removed lines while they are hidden —
/// and is what `v` and a message start from.
#[test]
fn the_line_cursor_moves_over_drawn_lines() {
    let mut diff = two_files();
    // Lines: a.txt's header 0, hunk 1, "-a" 2, "+b" 3, b.txt's header 4,
    // hunk 5, "-c" 6, "+d" 7. The headers' rules are rows of their own:
    // the rows are a.txt's header 0, its rule 1, hunk 2, "-a" 3, "+b" 4,
    // the rule above b.txt's header 5, the header 6, its rule 7, hunk 8,
    // "-c" 9, "+d" 10.
    diff.scroll.note_viewport(4);
    assert_eq!(diff.source(), None, "the combined diff is on no one file");
    diff.scroll.offset = 1;
    diff.apply(Action::ToggleLines);
    assert_eq!(
        diff.line_cursor,
        Some(2),
        "the first line of a hunk from the rule at the top"
    );
    diff.apply(Action::Down);
    assert_eq!(diff.line_cursor, Some(3));
    assert_eq!(diff.source(), Some(("a.txt", Some(1))));
    assert_eq!(diff.scroll.offset, 1, "still on screen");
    diff.apply(Action::Down);
    assert_eq!(diff.line_cursor, Some(6), "over b.txt's header and hunk");
    assert_eq!(diff.scroll.offset, 6, "scrolled to keep the cursor in view");
    assert_eq!(diff.source(), Some(("b.txt", Some(1))));
    diff.apply(Action::Last);
    assert_eq!(diff.line_cursor, Some(7));
    assert_eq!(diff.source(), Some(("b.txt", Some(1))));

    diff.apply(Action::Up);
    assert_eq!(diff.line_cursor, Some(6), "-c");
    diff.apply(Action::ToggleRemoved);
    assert_eq!(diff.line_cursor, Some(7), "-c is hidden; +d is next");
    diff.apply(Action::Up);
    assert_eq!(
        diff.line_cursor,
        Some(3),
        "over -c, the header and hunk to +b"
    );
    diff.apply(Action::First);
    assert_eq!(diff.line_cursor, Some(3), "-a is hidden too");

    diff.apply(Action::ToggleLines);
    assert_eq!(diff.line_cursor, None);
    let offset = diff.scroll.offset;
    diff.apply(Action::Down);
    assert_eq!(diff.scroll.offset, offset + 1, "j scrolls again");
}

/// In line selection the cursor keeps to added and removed lines,
/// across files in the combined diff and within the file in the per-file
/// view, stepping over removed lines too while they are hidden.
#[test]
fn line_selection_keeps_to_changed_lines() {
    let mut diff = two_files();
    // Lines: a.txt's header 0, hunk 1, " x" 2, "-a" 3, "+b" 4, hunk 5,
    // "-c" 6, " y" 7, b.txt's header 8, hunk 9, " z" 10, "+d" 11.
    diff.diff = Ok("diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n x\n-a\n+b\n\
                        @@ -9,2 +9 @@\n-c\n y\n\
                        diff --git a/b.txt b/b.txt\n@@ -1 +1,2 @@\n z\n+d\n"
        .into());
    diff.scroll.note_viewport(20);
    fn lines(diff: &mut CheckoutDiff, action: Action, expected: &[usize]) {
        for &expected in expected {
            diff.apply(action);
            assert_eq!(diff.line_cursor, Some(expected), "{action:?}");
        }
    }
    diff.apply(Action::ToggleLines);
    assert_eq!(diff.line_cursor, Some(3), "the first change from the top");
    lines(&mut diff, Action::Down, &[4, 6, 11, 11]);
    lines(&mut diff, Action::Up, &[6, 4, 3, 3]);

    // J and K go between the first lines of blocks of changes.
    lines(&mut diff, Action::FilesDown, &[6, 11, 11]);
    lines(&mut diff, Action::FilesUp, &[6, 3, 3]);

    diff.apply(Action::ToggleRemoved);
    assert_eq!(diff.line_cursor, Some(4), "off -a, now hidden");
    lines(&mut diff, Action::Down, &[11]);
    lines(&mut diff, Action::Up, &[4, 4]);
    // "-c" is a block of its own, hidden whole; "-a" "+b" starts at +b.
    lines(&mut diff, Action::FilesDown, &[11]);
    lines(&mut diff, Action::FilesUp, &[4]);
    diff.apply(Action::ToggleRemoved);

    diff.apply(Action::First);
    diff.apply(Action::ToggleFiles);
    assert_eq!((diff.selected_file, diff.line_cursor), (0, Some(3)));
    lines(&mut diff, Action::Last, &[6]);
    lines(&mut diff, Action::Down, &[6]);
    assert_eq!(diff.selected_file, 0, "the per-file view keeps to its file");
}

/// Switching between the combined diff and the per-file view keeps the
/// cursor on its line, counted in whichever patch is shown.
#[test]
fn the_line_cursor_goes_between_the_combined_and_per_file_views() {
    let mut diff = two_files();
    diff.line_cursor = Some(7);
    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.selected_file, 1);
    assert_eq!(diff.line_cursor, Some(3), "+d in b.txt's own patch");
    assert_eq!(diff.source(), Some(("b.txt", Some(1))));
    diff.apply(Action::Up);
    assert_eq!(diff.selected_file, 1, "j/k move the cursor, not the file");
    assert_eq!(diff.line_cursor, Some(2));
    diff.apply(Action::ToggleFiles);
    assert_eq!(diff.line_cursor, Some(6));

    diff.apply(Action::ToggleFiles);
    assert!(diff.leave_lines());
    assert!(!diff.leave_lines());
    assert_eq!(
        diff.source(),
        Some(("b.txt", None)),
        "the file being viewed"
    );
    diff.line_cursor = Some(1);
    diff.apply(Action::Search);
    assert_eq!(diff.line_cursor, None, "the filter may change the file");
    assert!(!diff.find.typing());
    diff.search_key(KeyEvent::from(KeyCode::Esc));
    diff.apply(Action::Find);
    assert!(!diff.find.typing(), "text is found in line selection only");
}

/// In line selection `f` finds text in the code as it is typed, from
/// where the cursor was, and Esc puts the cursor back there; once kept,
/// `n` and `N` go round the lines it is on, and Esc clears it before it
/// leaves line selection.
#[test]
fn found_text_is_stepped_through_with_n_and_shift_n() {
    let mut diff = two_files();
    // Lines: a.txt's header 0, hunk 1, " retry" 2, "-a" 3, "+Retry" 4,
    // hunk 5, " retry_backoff" 6, b.txt's header 7, hunk 8, "+retry" 9.
    diff.diff = Ok(
        "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n retry\n-a\n+Retry\n\
                        @@ -9 +9 @@\n retry_backoff\n\
                        diff --git a/retry.txt b/retry.txt\n@@ -1 +1 @@\n+retry\n"
            .into(),
    );
    diff.scroll.note_viewport(40);
    diff.apply(Action::ToggleLines);
    assert_eq!(diff.line_cursor, Some(3));
    let typed = |diff: &mut CheckoutDiff, text: &str| {
        for character in text.chars() {
            diff.find_key(KeyEvent::from(KeyCode::Char(character)));
        }
    };
    diff.apply(Action::Find);
    typed(&mut diff, "ret");
    assert_eq!(diff.line_cursor, Some(4), "found as it is typed");
    typed(&mut diff, "ry_");
    assert_eq!(diff.line_cursor, Some(6), "from where it was, not +Retry");
    // Ctrl-W takes back the word after the `_`, and the cursor with it.
    let control_w = KeyEvent::new(KeyCode::Char('w'), crossterm::event::KeyModifiers::CONTROL);
    typed(&mut diff, "bac");
    diff.find_key(control_w);
    assert_eq!(diff.find.query(), Some("retry_"));
    assert_eq!(diff.line_cursor, Some(6));
    diff.find_key(control_w);
    diff.find_key(control_w);
    assert_eq!(diff.find.query(), Some(""), "still being typed");
    assert_eq!(diff.line_cursor, Some(3), "back where it was");
    typed(&mut diff, "retry_");
    diff.find_key(KeyEvent::from(KeyCode::Esc));
    assert_eq!((diff.line_cursor, diff.find.query()), (Some(3), None));

    diff.apply(Action::Find);
    typed(&mut diff, "retry");
    diff.find_key(KeyEvent::from(KeyCode::Enter));
    assert_eq!(diff.find.query(), Some("retry"));
    let mut step = |action, expected| {
        diff.apply(action);
        assert_eq!(diff.line_cursor, Some(expected), "{action:?}");
    };
    step(Action::FindNext, 6);
    step(Action::FindNext, 9);
    step(Action::FindNext, 2);
    step(Action::FindPrevious, 9);
    step(Action::FindPrevious, 6);

    diff.apply(Action::Find);
    typed(&mut diff, "Retry");
    diff.find_key(KeyEvent::from(KeyCode::Enter));
    diff.apply(Action::FindNext);
    diff.apply(Action::FindNext);
    assert_eq!(diff.line_cursor, Some(4), "a capital is matched as typed");

    assert!(diff.clear_find());
    assert_eq!(diff.line_cursor, Some(4));
    assert!(!diff.clear_find());
}

/// Found text is marked where it is drawn — on the line cursor's line as
/// the current match — and the prompt counts which of its lines the
/// cursor is on.
#[cfg(feature = "renderer")]
#[test]
fn found_text_is_marked_and_counted() {
    let mut diff = two_files();
    diff.apply(Action::ToggleLines);
    diff.apply(Action::Find);
    for key in [KeyCode::Char('d'), KeyCode::Enter] {
        diff.find_key(KeyEvent::from(key));
    }
    assert_eq!(diff.line_cursor, Some(7), "+d");
    for side_by_side in [false, true] {
        diff.side_by_side = side_by_side;
        let buffer = draw_frame(120, 20, |frame| {
            crate::render(frame, &diff.view(Block::default()), frame.area());
        });
        let cursor_rows: Vec<_> = buffer_rows(&buffer)
            .into_iter()
            .filter(|row| row.starts_with('▌'))
            .collect();
        assert_eq!(cursor_rows.len(), 1, "side by side: {side_by_side}");
        assert!(cursor_rows[0].contains('d'), "{}", cursor_rows[0]);
        let marked = |style: ratatui::style::Style| -> Vec<(u16, &str)> {
            (0..20)
                .flat_map(|y| (0..120).map(move |x| (x, y)))
                .filter(|&at| Some(buffer[at].bg) == style.bg)
                .filter(|&at| style.fg.is_none() || Some(buffer[at].fg) == style.fg)
                .map(|(_, y)| (y, buffer[(0, y)].symbol()))
                .collect()
        };
        let current = marked(crate::find::current_style());
        assert_eq!(current.len(), 1, "only d, not b.txt's name: {current:?}");
        assert_eq!(current[0].1, "▌", "on the cursor's row");
        assert!(
            marked(ratatui::style::Style::new().bg(styra_ui::theme::ACCENT)).is_empty(),
            "no other match"
        );
        let rows: Vec<String> = buffer_rows(&buffer);
        assert!(rows.iter().any(|row| row.contains("/d  [1/1]")), "{rows:?}");
    }
}

#[test]
fn a_standing_filter_is_cleared_before_anything_else() {
    let mut diff = two_files();
    assert!(
        !diff.clear_filter(),
        "nothing to clear in the combined diff"
    );
    diff.apply(Action::ToggleFiles);
    diff.apply(Action::Search);
    diff.search_key(KeyEvent::from(KeyCode::Char('b')));
    assert_eq!(diff.selected_file, 1, "the selection follows the filter");
    diff.search_key(KeyEvent::from(KeyCode::Enter));

    assert!(diff.clear_filter());
    assert_eq!(diff.search.query(), None);
    assert!(!diff.clear_filter());
}
