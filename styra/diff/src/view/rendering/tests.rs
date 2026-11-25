use super::*;
use crate::test_support::{draw as draw_frame, rows as buffer_rows};
use ratatui::{backend::TestBackend, Terminal};

const DIFF: &str = "\
diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
 fn a() {}
-fn b() {}
+fn c() {}
+fn d() {}
";

#[test]
fn shortcut_rows_only_appear_in_the_legend() {
    for help in [None, Some(0), Some(u16::MAX)] {
        let buffer = draw_frame(100, 20, |frame| {
            let mut view = CheckoutDiffView::unread(Block::bordered());
            view.diff = Ok(DIFF);
            view.help = help;
            render(frame, &view, frame.area());
        });
        let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        if help.is_some() {
            assert!(output.contains("Shortcuts"), "{output}");
            if help == Some(0) {
                assert!(output.contains("toggle unified / side-by-side"), "{output}");
            } else {
                assert!(output.contains("close the diff"), "{output}");
            }
        } else {
            assert!(!output.contains("s:"), "{output}");
            assert!(!output.contains("v: edit file"), "{output}");
            assert!(output.contains("fn c()"), "{output}");
        }
    }
}

#[test]
fn file_view_shows_the_title_and_hunks_without_git_headers() {
    for side_by_side in [false, true] {
        let buffer = draw_frame(140, 24, |frame| {
            let mut view = CheckoutDiffView::unread(Block::bordered());
            view.diff = Ok(DIFF);
            view.per_file = true;
            view.side_by_side = side_by_side;
            let measured = render(frame, &view, frame.area());
            assert_eq!(measured.side_by_side, side_by_side);
        });
        let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(output.contains("src/a.rs [M]"), "{output}");
        assert!(output.contains("┄┄┄"), "{output}");
        assert!(output.contains("fn c() {}"), "{output}");
        assert!(
            output.find("End of file").unwrap() > output.find("fn d() {}").unwrap(),
            "{output}"
        );
        for metadata in ["diff --git", "index 1111111", "--- a/", "+++ b/"] {
            assert!(!output.contains(metadata), "{output}");
        }
    }
    let (combined, _) = screen(Ok(DIFF), None, 0);
    assert!(!combined.contains("End of file"), "{combined}");
    assert!(combined.contains("src/a.rs"), "{combined}");
    for metadata in ["diff --git", "index 1111111", "--- a/", "+++ b/"] {
        assert!(!combined.contains(metadata), "{combined}");
    }
}

#[test]
fn file_view_retains_changes_without_hunks() {
    for detail in [
        "Binary files a/image.png and b/image.png differ",
        "old mode 100644\nnew mode 100755",
        "similarity index 100%\nrename from old\nrename to new",
    ] {
        let text = format!("diff --git a/old b/new\n{detail}\n");
        let shown = patch_lines(
            &text,
            Hidden::default(),
            Tint::Full,
            true,
            None,
            80,
            true,
            None,
        );
        let shown = shown
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(shown, detail);
    }
}

/// With layers, each change is banded across the panel in its layers'
/// blend instead of green or red, removed rows are still known to hide,
/// and unchanged rows are left alone.
#[test]
fn layers_tint_each_change_by_the_commits_that_made_it() {
    use crate::layers::{background, Layer};
    let layer = |name: &str| Layer {
        name: name.into(),
        ..Layer::default()
    };
    let first = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                     @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n";
    let second = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                      @@ -1,2 +1,3 @@\n fn a() {}\n-fn c() {}\n+fn c() {}\n+fn d() {}\n";
    let layers = Layers::from_steps(&[(layer("one"), first.into()), (layer("two"), second.into())]);
    let backgrounds = |hide_removed| {
        patch_lines(
            DIFF,
            Hidden::removed(hide_removed),
            Tint::Full,
            false,
            Some(&layers),
            40,
            false,
            None,
        )
        .into_iter()
        .map(|line| {
            assert!(line.width() == 40 || line.style.bg.is_none());
            line.style.bg
        })
        .collect::<Vec<_>>()
    };
    let (removed, both) = (background(&[0], true), background(&[0, 1], false));
    assert_eq!(
        &backgrounds(false)[3..],
        [None, removed, both, both],
        " fn a, -fn b, +fn c, +fn d"
    );
    assert_eq!(&backgrounds(true)[3..], [None, both, both]);
}

/// On a line in line selection, the commits that touched it are listed
/// at the bottom, newest first; on a line none changed, that is said.
#[test]
fn the_line_cursors_commits_are_listed_newest_first_at_the_bottom() {
    use crate::layers::Layer;
    use ratatui::{backend::TestBackend, Terminal};
    let layer = |name: &str, summary: &str| Layer {
        name: name.into(),
        summary: summary.into(),
        author: "Ada".into(),
        date: "2 days ago".into(),
        body: String::new(),
    };
    let first = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                     @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n";
    let second = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                      @@ -1,2 +1,3 @@\n fn a() {}\n-fn c() {}\n+fn c() {}\n+fn d() {}\n";
    let second_layer = Layer {
        body: (1..=12).map(|n| format!("body {n}\n")).collect(),
        ..layer("2222222", "second")
    };
    let layers = Layers::from_steps(&[
        (layer("1111111", "first"), first.into()),
        (second_layer, second.into()),
    ]);
    let bottom = |cursor: usize| {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal
            .draw(|frame| {
                let mut view = CheckoutDiffView::unread(Block::default());
                view.diff = Ok(DIFF);
                view.layers = Some(Ok(&layers));
                view.line_cursor = Some(cursor);
                render(frame, &view, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (28..40)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    // The diff's side, left of the legend's column.
                    .split('│')
                    .next()
                    .unwrap_or_default()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
    };
    // Line 7 is "+fn c() {}", laid by the second commit over the first.
    // The strip is as tall as both commits' messages: a rule, the
    // second commit's row and nine of its body's twelve lines, so ten
    // lines of message, then the first commit's row.
    let rows = bottom(7);
    assert!(rows[0].contains('─'), "{rows:?}");
    assert!(
        rows[1].ends_with("2222222 2 days ago · Ada  second"),
        "{rows:?}"
    );
    assert!(rows[2] == "   body 1", "{rows:?}");
    assert!(rows[10] == "   body 9 …", "{rows:?}");
    assert!(
        rows[11].ends_with("1111111 2 days ago · Ada  first"),
        "{rows:?}"
    );
    // On a line no commit changed, the strip is only as tall as saying so.
    let rows = bottom(5);
    assert!(rows[10].contains('─'), "{rows:?}");
    assert_eq!(rows[11], "no commit changed this line", "{rows:?}");
    assert!(rows[..10].iter().all(|row| !row.contains('─')), "{rows:?}");

    // Where the strip grows over the cursor's line, the diff scrolls to
    // keep it in sight.
    let mut terminal = Terminal::new(TestBackend::new(100, 14)).unwrap();
    let mut measured = CheckoutDiffFeedback::default();
    terminal
        .draw(|frame| {
            let mut view = CheckoutDiffView::unread(Block::default());
            view.diff = Ok(DIFF);
            view.layers = Some(Ok(&layers));
            view.line_cursor = Some(7);
            measured = render(frame, &view, frame.area());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let cursor = (0..14).find(|&y| buffer[(0, y)].symbol() == LINE_CURSOR_MARKER);
    assert!(cursor.is_some(), "the cursor is drawn: {buffer:?}");
    assert!(measured.scroll.effective_scroll > 0);
}

/// Files sharing the focused file's or line block's commits stand out
/// in the file list, including files moved without edits.
#[test]
fn the_files_the_cursors_commits_changed_stand_out() {
    use crate::layers::Layer;
    let layer = |name: &str| Layer {
        name: name.into(),
        ..Layer::default()
    };
    let moved = "diff --git a/old.txt b/moved.txt\nsimilarity index 100%\nrename from old.txt\nrename to moved.txt\n";
    let text = format!(
        "{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old\n+new\n\
             diff --git a/c.txt b/c.txt\n@@ -1 +1 @@\n-x\n+y\n{moved}"
    );
    let one = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                   @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n\
                   diff --git a/c.txt b/c.txt\n--- a/c.txt\n+++ b/c.txt\n@@ -1 +1 @@\n-x\n+y\n";
    let two = "diff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let layers = Layers::from_steps(&[
        (layer("one"), format!("{one}{moved}")),
        (layer("two"), two.into()),
    ]);
    // The style of each listed file's name, with the cursor on `line` of
    // the patch of the file at `selected` in the diff.
    let names = |selected: usize, line: Option<usize>| {
        let buffer = draw_frame(100, 24, |frame| {
            let mut view = CheckoutDiffView::unread(Block::default());
            view.diff = Ok(&text);
            view.per_file = true;
            view.selected_file = selected;
            view.layers = Some(Ok(&layers));
            view.line_cursor = line;
            render(frame, &view, frame.area());
        });
        ["a.rs", "b.txt", "c.txt", "moved.txt"].map(|name| {
            (0..24)
                .find_map(|y| {
                    let row: String = (0..50).map(|x| buffer[(x, y)].symbol()).collect();
                    let x = row.find(name)?;
                    let x = u16::try_from(row[..x].chars().count()).unwrap();
                    // Only the file list's rows, not the patch's title.
                    (row.contains('│') && row.find('│')? > row.find(name)?)
                        .then(|| buffer[(x, y)].style())
                })
                .unwrap_or_else(|| panic!("{name} listed: {buffer:?}"))
        })
    };
    let standing = |style: Style| style.add_modifier.contains(Modifier::BOLD);
    let muted = |style: Style| style.fg == Some(theme::MUTED_TEXT);

    // On b.txt's "+new", which only the second commit wrote: c.txt and
    // a.rs, only the first's, are muted.
    let [a, _, c, moved] = names(1, Some(3));
    assert!(
        muted(a) && muted(c) && muted(moved),
        "{a:?} {c:?} {moved:?}"
    );
    // On a.rs's "+fn c() {}", the first commit's: c.txt stands out too.
    let [_, b, c, moved] = names(0, Some(7));
    assert!(standing(c) && standing(moved), "{c:?} {moved:?}");
    assert!(muted(b), "{b:?}");
    // Without a line cursor, use all commits contributing to the file.
    let [_, b, c, moved] = names(0, None);
    assert!(
        standing(c) && standing(moved) && muted(b),
        "{b:?} {c:?} {moved:?}"
    );
    // A move without edits also focuses the other files in its commit.
    let [a, b, c, _] = names(3, None);
    assert!(standing(a) && standing(c) && muted(b), "{a:?} {b:?} {c:?}");
    let [a, _, c, moved] = names(1, None);
    assert!(
        muted(a) && muted(c) && muted(moved),
        "{a:?} {c:?} {moved:?}"
    );
    // On an unchanged line, nothing is singled out.
    let [_, b, c, moved] = names(0, Some(5));
    for style in [b, c, moved] {
        assert!(!standing(style) && !muted(style), "{style:?}");
    }
}

/// The legend lists the layers a row each: in the per-file view in the
/// left pane under the file list, and in the combined diff, which has no
/// such pane, at the top left of the top bar before the header. In
/// line selection the commits that touched the block of changes under
/// the cursor stand out, and the others are muted.
#[test]
fn the_layers_legend_lists_each_commit_at_the_top_left() {
    use crate::layers::{background, Layer};
    let layer = |name: &str, summary: &str| Layer {
        name: name.into(),
        summary: summary.into(),
        ..Layer::default()
    };
    let step = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n\
                    @@ -1,2 +1,2 @@\n fn a() {}\n-fn b() {}\n+fn c() {}\n";
    let layers = Layers::from_steps(&[
        (layer("1111111", "first"), step.into()),
        (layer("uncommitted", ""), String::new()),
    ]);
    for (per_file, cursor) in [
        (false, None),
        (true, None),
        (false, Some(8)),
        (false, Some(5)),
    ] {
        let buffer = draw_frame(100, 24, |frame| {
            let mut view = CheckoutDiffView::unread(Block::default());
            view.diff = Ok(DIFF);
            view.per_file = per_file;
            view.layers = Some(Ok(&layers));
            view.line_cursor = cursor;
            render(frame, &view, frame.area());
        });
        let row = |y: u16| -> String { (0..100).map(|x| buffer[(x, y)].symbol()).collect() };
        let (top, left) = (0..24)
            .find_map(|y| Some((y, row(y).find("layers, oldest first:")?)))
            .unwrap_or_else(|| panic!("no legend in {buffer:?}"));
        let x = u16::try_from(row(top)[..left].chars().count()).unwrap();
        let border = (0..100)
            .find(|&x| buffer[(x, top)].symbol() == "│")
            .expect("a rule beside the legend");
        let rule = (0..24).find(|&y| row(y).starts_with('─')).unwrap();
        if per_file {
            assert_eq!(x, 0, "in the left pane");
            assert!(top > rule, "below the top bar's rule, at {rule}");
            assert!(
                row(top - 1).contains("files involved · c:"),
                "count above the commits"
            );
            assert!(row(top - 2).starts_with('─'), "a rule above the count");
            let above: String = (rule..top)
                .flat_map(|y| row(y).chars().take(usize::from(border)).collect::<Vec<_>>())
                .collect();
            assert!(above.contains("a.rs"), "the file list is above: {above}");
        } else {
            assert_eq!((x, top), (0, 0), "at the top left, in the top bar");
            let header: String = row(top).chars().skip(usize::from(border) + 1).collect();
            assert!(header.starts_with(" detached head"), "{header}");
            assert!(
                rule > top + 2,
                "the legend is above the bar's rule, at {rule}"
            );
        }
        let at = |y: u16| -> String { (x..100).map(|x| buffer[(x, y)].symbol()).collect() };
        assert!(
            at(top + 1).starts_with("   1111111 first"),
            "{}",
            at(top + 1)
        );
        assert!(at(top + 2).starts_with("   uncommitted"), "{}", at(top + 2));
        for (offset, index) in [(1, 0), (2, 1)] {
            assert_eq!(
                buffer[(x, top + offset)].bg,
                background(&[index], false).unwrap()
            );
        }
        let (first, uncommitted) = (&buffer[(x + 3, top + 1)], &buffer[(x + 3, top + 2)]);
        if cursor == Some(8) || cursor.is_none() {
            // "+fn d() {}", in the block "-fn b() {}" "+fn c() {}" begins,
            // which only the first commit touched.
            assert_eq!(first.bg, theme::SELECTION_BACKGROUND);
            assert!(first.modifier.contains(Modifier::BOLD));
            assert_eq!(
                buffer[(border - 1, top + 1)].bg,
                theme::SELECTION_BACKGROUND,
                "banded to the rule"
            );
            assert_eq!(uncommitted.fg, theme::MUTED_TEXT);
            assert_ne!(uncommitted.bg, theme::SELECTION_BACKGROUND);
        } else {
            // On no block, as on " fn a() {}", or with no cursor.
            assert_eq!(first.fg, theme::ACCENT);
            assert_eq!(uncommitted.fg, theme::ACCENT);
            assert_ne!(first.bg, theme::SELECTION_BACKGROUND);
        }
    }
}

fn commit_cell<'a>(buffer: &'a ratatui::buffer::Buffer, name: &str) -> &'a ratatui::buffer::Cell {
    (buffer.area.top()..buffer.area.bottom())
        .find_map(|y| {
            let row: String = (buffer.area.left()..buffer.area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            row.find(name)
                .map(|x| &buffer[(buffer.area.left() + x as u16, y)])
        })
        .expect("commit listed")
}

#[test]
fn file_focus_highlights_all_its_commits_and_follows_file_changes() {
    use crate::layers::Layer;
    let first = "diff --git a/a.txt b/a.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let second = "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n new\n-before\n+after\n";
    let other = "diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-x\n+y\n";
    let layers = Layers::from_steps(
        &[first, second, other]
            .into_iter()
            .enumerate()
            .map(|(i, patch)| {
                (
                    Layer {
                        name: format!("commit{i}"),
                        ..Layer::default()
                    },
                    patch.to_owned(),
                )
            })
            .collect::<Vec<_>>(),
    );
    let text = format!(
        "diff --git a/a.txt b/a.txt\n@@ -1,2 +1,2 @@\n-old\n+new\n-before\n+after\n{other}"
    );
    for selected in [0, 1] {
        let buffer = draw_frame(100, 24, |frame| {
            let mut view = CheckoutDiffView::unread(Block::default());
            view.diff = Ok(&text);
            view.per_file = true;
            view.selected_file = selected;
            view.layers = Some(Ok(&layers));
            render(frame, &view, frame.area());
        });
        for index in 0..3 {
            let name = format!("commit{index}");
            let cell = commit_cell(&buffer, &name);
            let highlighted = if selected == 0 { index < 2 } else { index == 2 };
            assert_eq!(
                cell.bg == theme::SELECTION_BACKGROUND,
                highlighted,
                "{name}, file {selected}"
            );
            assert_eq!(cell.modifier.contains(Modifier::BOLD), highlighted);
        }
    }
}

#[test]
fn file_focus_highlights_a_move_without_edits() {
    use crate::layers::Layer;
    let moved = "diff --git a/old.txt b/new.txt\nsimilarity index 100%\nrename from old.txt\nrename to new.txt\n";
    let other = "diff --git a/other.txt b/other.txt\n@@ -1 +1 @@\n-old\n+new\n";
    let layers = Layers::from_steps(&[
        (
            Layer {
                name: "move_commit".into(),
                ..Layer::default()
            },
            moved.into(),
        ),
        (
            Layer {
                name: "edit_commit".into(),
                ..Layer::default()
            },
            other.into(),
        ),
    ]);
    let text = format!("{moved}{other}");
    for per_file in [false, true] {
        for files_below in [false, true] {
            let buffer = draw_frame(100, 24, |frame| {
                let mut view = CheckoutDiffView::unread(Block::default());
                view.diff = Ok(&text);
                view.per_file = per_file;
                view.files_below = files_below;
                view.layers = Some(Ok(&layers));
                render(frame, &view, frame.area());
            });
            for (name, highlighted) in [("move_commit", true), ("edit_commit", false)] {
                let cell = commit_cell(&buffer, name);
                assert_eq!(
                    cell.bg == theme::SELECTION_BACKGROUND,
                    highlighted,
                    "{name}, per_file={per_file}, files_below={files_below}"
                );
                assert_eq!(cell.modifier.contains(Modifier::BOLD), highlighted);
            }
        }
    }
}

#[test]
fn hiding_removals_keeps_added_indicators_and_original_line_numbers() {
    let lines = patch_lines(
        DIFF,
        Hidden::removed(true),
        Tint::Full,
        true,
        None,
        80,
        false,
        None,
    );
    let text = lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("fn b()"), "{text}");
    assert!(text.contains("1  fn a()"), "{text}");
    assert!(text.contains("2 +fn c()"), "{text}");
    assert!(text.contains("3 +fn d()"), "{text}");
    assert!(!text.contains("--- a/src/a.rs"), "{text}");
    assert!(text.contains("src/a.rs"), "{text}");
    assert_eq!(
        lines
            .iter()
            .filter(|line| line.style.bg == Some(theme::DIFF_ADDED_BACKGROUND))
            .count(),
        2
    );
    assert!(patch_lines(
        DIFF,
        Hidden::default(),
        Tint::Full,
        true,
        None,
        80,
        false,
        None
    )
    .iter()
    .any(|line| line.style.bg == Some(theme::DIFF_REMOVED_BACKGROUND)));
}

/// The per-file view's left pane holds the files, then the layers, then
/// the thumbnail at the bottom; put away, the thumbnail leaves the rest.
#[test]
fn the_left_pane_lists_files_then_layers_then_the_thumbnail() {
    use crate::layers::Layer;
    let layer = |name: &str| Layer {
        name: name.into(),
        ..Layer::default()
    };
    let layers = Layers::from_steps(&[(layer("1111111"), String::new())]);
    let draw = |hidden| {
        let buffer = draw_frame(100, 40, |frame| {
            let mut view = CheckoutDiffView::unread(Block::default());
            view.diff = Ok(DIFF);
            view.per_file = true;
            view.thumbnail = true;
            view.thumbnail_hidden = hidden;
            view.layers = Some(Ok(&layers));
            render(frame, &view, frame.area());
        });
        buffer
    };
    let first_row = |buffer: &ratatui::buffer::Buffer, text: &str| {
        (0..40).find(|&y| {
            (0..30)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains(text)
        })
    };
    let map = |buffer: &ratatui::buffer::Buffer| {
        (0..40).find(|&y| {
            (0..30).any(|x| {
                [theme::DIFF_MAP_ADDED, theme::DIFF_MAP_REMOVED].contains(&buffer[(x, y)].bg)
            })
        })
    };
    let shown = draw(false);
    let files = first_row(&shown, "a.rs").expect("the file list");
    let legend = first_row(&shown, "layers, oldest first:").expect("the layers");
    let thumbnail = map(&shown).expect("the thumbnail");
    assert!(
        files < legend && legend < thumbnail,
        "{files} {legend} {thumbnail}"
    );

    let hidden = draw(true);
    assert_eq!(map(&hidden), None, "the thumbnail is put away");
    let legend = first_row(&hidden, "layers, oldest first:").expect("the layers");
    assert!(first_row(&hidden, "a.rs").unwrap() < legend);
}

#[test]
fn fullscreen_thumbnail_follows_the_selected_file_and_fits_beneath_the_tree() {
    let patch = "diff --git a/added.rs b/added.rs\n@@ -0,0 +1 @@\n+    added\n\
                     diff --git a/deleted.rs b/deleted.rs\n@@ -1 +0,0 @@\n-removed\n";
    let draw = |thumbnail, per_file, selected_file, width, height| {
        let buffer = draw_frame(width, height, |frame| {
            render(
                frame,
                &CheckoutDiffView {
                    diff: Ok(patch),
                    thumbnail,
                    per_file,
                    selected_file,
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
        });
        buffer
    };
    let added = draw(true, true, 0, 80, 30);
    let removed = draw(true, true, 1, 80, 30);
    let pixels = |buffer: &ratatui::buffer::Buffer, color| {
        buffer
            .content()
            .iter()
            .filter(|cell| {
                cell.bg
                    == if color == theme::SUCCESS {
                        theme::DIFF_MAP_ADDED
                    } else {
                        theme::DIFF_MAP_REMOVED
                    }
            })
            .count()
    };
    assert!(pixels(&added, theme::SUCCESS) > 0);
    assert_eq!(pixels(&added, theme::ERROR), 0);
    assert!(pixels(&removed, theme::ERROR) > 0);
    assert_eq!(pixels(&removed, theme::SUCCESS), 0);
    // Image stays in the lower-left, underneath the selected file.
    let (image_x, image_y) = added.pos_of(
        added
            .content()
            .iter()
            .position(|cell| cell.bg == theme::DIFF_MAP_ADDED)
            .unwrap(),
    );
    assert!(image_x < 30 && image_y > 7);
    for buffer in [
        draw(false, true, 0, 80, 30),
        draw(true, false, 0, 80, 30),
        draw(true, true, 0, 20, 10),
    ] {
        let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(!output.contains("1 lines"));
    }
}

#[test]
fn fifteen_sections_preserve_edits_inside_the_viewport_outline() {
    let buffer = draw_frame(30, 19, |frame| {
        render_thumbnail(
            frame,
            "@@ -10 +10 @@\n-old\n+new\n@@ -900 +900 @@\n-old\n+new\n",
            Some(1000),
            frame.area(),
            Hidden::default(),
            0,
            100,
            false,
        );
    });
    let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
    assert!(output.contains("1000 lines"));
    assert!(output.contains("+2") && output.contains("−2"));
    assert!(!output.contains("501"), "sections have no line numbers");
    // All 15 sections are contiguous filled rows, including the viewport rules.
    for y in 1..16 {
        for x in 1..29 {
            assert!([
                theme::DIFF_MAP_ADDED,
                theme::DIFF_MAP_REMOVED,
                theme::SELECTION_BACKGROUND
            ]
            .contains(&buffer[(x, y)].bg));
        }
    }
    let unchanged = &buffer[(2, 9)];
    assert_eq!(unchanged.bg, theme::SELECTION_BACKGROUND);
    assert!(buffer
        .content()
        .iter()
        .any(|cell| cell.bg == theme::DIFF_MAP_ADDED));
    assert!(buffer
        .content()
        .iter()
        .any(|cell| cell.bg == theme::DIFF_MAP_REMOVED));
    assert_eq!(
        buffer
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "┌" && cell.fg == theme::DIRECTORY_NAME)
            .count(),
        1
    );
}

#[test]
fn changed_sections_fill_the_width_and_split_by_edit_ratio() {
    let patch = format!(
            "@@ -1,3 +1,1 @@\n{}+new\n@@ -101,1 +101,3 @@\n-old\n{}@@ -201,0 +201 @@\n+only addition\n@@ -301 +301,0 @@\n-only deletion\n",
            "-old\n".repeat(3), "+new\n".repeat(3)
        );
    let pixels = thumbnail_pixels(&patch, Some(1500), 20, 15);
    for (section, red) in [(0, 15), (1, 5), (2, 0), (3, 20)] {
        let row = &pixels[section * 20..(section + 1) * 20];
        assert!(row[..red].iter().all(|&pixel| pixel == theme::ERROR));
        assert!(row[red..].iter().all(|&pixel| pixel == theme::SUCCESS));
    }
    assert!(pixels[4 * 20..]
        .iter()
        .all(|&pixel| pixel == theme::CODE_BACKGROUND));
}

#[test]
fn deleted_file_uses_original_length_and_positions() {
    let patch = format!(
        "deleted file mode 100644\n+++ /dev/null\n@@ -1,1000 +0,0 @@\n{}",
        "-old\n".repeat(1000)
    );
    let (extent, _) = thumbnail_line_positions(&patch);
    assert_eq!(extent, 1000);
    let pixels = thumbnail_pixels(&patch, None, 2, 10);
    for band in pixels.chunks_exact(2) {
        assert_eq!(band, [theme::ERROR, theme::ERROR]);
    }
}

#[test]
fn viewport_box_moves_with_scrolling_and_keeps_edit_colors() {
    let patch = format!(
        "diff --git a/file.rs b/file.rs\n@@ -10,0 +10,40 @@\n{}@@ -761,0 +801,40 @@\n{}",
        "+early\n".repeat(40),
        "+late\n".repeat(40)
    );
    let draw = |scroll| {
        let buffer = draw_frame(80, 30, |frame| {
            render(
                frame,
                &CheckoutDiffView {
                    diff: Ok(&patch),
                    per_file: true,
                    thumbnail: true,
                    requested_scroll: scroll,
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
        });
        buffer
    };
    let top = draw(0);
    let bottom = draw(u16::MAX);
    let markers = |buffer: &ratatui::buffer::Buffer| {
        buffer
            .content()
            .iter()
            .enumerate()
            .filter(|(_, cell)| {
                ["┌", "└"].contains(&cell.symbol()) && cell.fg == theme::DIRECTORY_NAME
            })
            .map(|(index, _)| buffer.pos_of(index).1)
            .collect::<Vec<_>>()
    };
    let early = markers(&top);
    let late = markers(&bottom);
    assert!(!early.is_empty() && !late.is_empty());
    assert!(early.last().unwrap() < late.first().unwrap());
    for buffer in [&top, &bottom] {
        assert!(buffer
            .content()
            .iter()
            .any(|cell| cell.bg == theme::DIFF_MAP_ADDED));
    }
}

#[test]
fn a_single_box_encloses_disjoint_visible_areas() {
    let buffer = draw_frame(12, 17, |frame| {
        render_viewport_box(
            frame,
            frame.area(),
            &[false, true, false, false, false, true, false, false],
        );
    });
    for (position, glyph) in [
        ((0, 1), "┌"),
        ((11, 1), "┐"),
        ((0, 5), "└"),
        ((11, 5), "┘"),
        ((0, 4), "│"),
        ((11, 4), "│"),
        ((5, 1), "─"),
        ((5, 5), "─"),
    ] {
        assert_eq!(buffer[position].symbol(), glyph);
    }
    assert_eq!(
        buffer
            .content()
            .iter()
            .filter(|cell| cell.symbol() == "┌")
            .count(),
        1
    );
    assert_eq!(buffer[(5, 4)].symbol(), " ", "gaps remain inside the box");
}

#[test]
fn viewport_mapping_accounts_for_metadata_rules_and_hidden_removals() {
    let patch = "diff --git a/file b/file\n@@ -10,2 +10,2 @@\n-old\n+new\n same\n@@ -800 +800 @@\n-old\n+new\n";
    // Header, synthetic rule and hunk header have no file position.
    assert!(thumbnail_viewport(patch, Some(1000), false, 0, 3, 10)
        .iter()
        .all(|&visible| !visible));
    let early = thumbnail_viewport(patch, Some(1000), false, 3, 1, 10);
    assert!(early[0]);
    assert_eq!(early, thumbnail_viewport(patch, Some(1000), true, 3, 1, 10));
    let late = thumbnail_viewport(patch, Some(1000), false, 6, 3, 10);
    assert!(late[7]);
    assert_eq!(late.iter().filter(|&&visible| visible).count(), 1);
    assert_eq!(late, thumbnail_viewport(patch, Some(1000), true, 6, 1, 10));
}

#[test]
fn edit_map_places_changes_in_the_whole_file_including_unchanged_gaps_and_tail() {
    let patch = "@@ -10 +10 @@\n-old\n+new\n@@ -801,0 +801,1 @@\n+new\n";
    let pixels = thumbnail_pixels(patch, Some(1000), 10, 10);
    assert_eq!(&pixels[0..5], &[theme::ERROR; 5]);
    assert_eq!(&pixels[5..10], &[theme::SUCCESS; 5]);
    assert!(pixels[10..80]
        .iter()
        .all(|&pixel| pixel == theme::CODE_BACKGROUND));
    assert_eq!(&pixels[80..90], &[theme::SUCCESS; 10]);
    assert!(pixels[90..]
        .iter()
        .all(|&pixel| pixel == theme::CODE_BACKGROUND));
}

#[test]
fn nearby_additions_collapse_to_one_marker_independent_of_code_shape() {
    let one = thumbnail_pixels("@@ -101,0 +101 @@\n+x\n", Some(1000), 10, 10);
    let ten = format!("@@ -101,0 +101,10 @@\n{}", "+    longer code\n".repeat(10));
    assert_eq!(one, thumbnail_pixels(&ten, Some(1000), 10, 10));
    assert!(thumbnail_pixels("Binary files differ", None, 10, 4)
        .iter()
        .all(|&pixel| pixel == theme::CODE_BACKGROUND));
    assert!(thumbnail_pixels(&ten, Some(1000), 0, 4).is_empty());
}

fn screen(diff: Result<&str, &str>, base_branch: Option<&str>, scroll: u16) -> (String, u16) {
    file_screen(diff, base_branch, scroll, false, 0).0
}

fn file_screen(
    diff: Result<&str, &str>,
    base_branch: Option<&str>,
    scroll: u16,
    per_file: bool,
    selected_file: usize,
) -> ((String, u16), u16) {
    filtered_screen(
        diff,
        base_branch,
        scroll,
        per_file,
        selected_file,
        Default::default(),
    )
}

fn filtered_screen(
    diff: Result<&str, &str>,
    base_branch: Option<&str>,
    scroll: u16,
    per_file: bool,
    selected_file: usize,
    search: SearchView<'_>,
) -> ((String, u16), u16) {
    let (width, height) = (80, 18);
    let mut measured = CheckoutDiffFeedback::default();
    let buffer = draw_frame(width, height, |frame| {
        measured = render(
            frame,
            &CheckoutDiffView {
                worktree: Path::new("/state/worktrees/styra-7"),
                branch: Some("styra/rename"),
                base_branch,
                base_commit: "4bf5c35d0123456789abcdef",
                branch_point: true,
                diff,
                requested_scroll: scroll,
                per_file,
                selected_file,
                search,
                ..CheckoutDiffView::unread(Block::bordered())
            },
            frame.area(),
        );
    });
    let output = buffer_rows(&buffer).join("\n");
    ((output, measured.scroll.limit), measured.scroll.viewport)
}

/// The per-file view with `report`, 100 columns wide: the screen, and the
/// rows left for the patch.
fn report_screen(report: &ReportState, per_file: bool) -> (String, u16) {
    let (width, height) = (100, 24);
    let mut viewport = 0;
    let buffer = draw_frame(width, height, |frame| {
        let mut view = CheckoutDiffView::unread(Block::bordered());
        view.diff = Ok(DIFF);
        view.per_file = per_file;
        view.report = Some(report);
        viewport = render(frame, &view, frame.area()).scroll.viewport;
    });
    let output = buffer_rows(&buffer).join("\n");
    (output, viewport)
}

/// The selected file's description sits between its name and its patch,
/// saying when the file has changed since; a report on its way says so,
/// and a hidden one gives its rows back to the patch.
#[test]
fn the_per_file_view_describes_the_selected_file_from_the_report() {
    let (_, without) = report_screen(&ReportState::default(), true);
    let (output, _) = report_screen(&ReportState::default(), true);
    assert!(!output.contains("E:"), "{output}");
    assert!(!output.contains("e: hide"), "{output}");

    let described = |patch: &str| ReportState {
        report: Some(crate::Report {
            files: [(
                "src/a.rs".to_owned(),
                crate::FileReport {
                    fingerprint: crate::report::fingerprint(patch),
                    description: "**Replaces** `b` with `c` and `d`, so callers get both.".into(),
                },
            )]
            .into(),
            failures: Vec::new(),
        }),
        ..Default::default()
    };
    let (output, with) = report_screen(&described(DIFF), true);
    assert!(
        output.contains("Replaces b with c and d, so callers get both."),
        "{output}"
    );
    assert!(!output.contains("written before"), "{output}");
    assert!(!output.contains("e: hide edit report"));
    assert_eq!(with, without - 2, "a line of description and its rule");
    let description = output.find("Replaces b").unwrap();
    assert!(output.find("src/a.rs [M]").unwrap() < description);
    assert!(description < output.find("fn c()").unwrap(), "{output}");

    let (output, _) = report_screen(&described("an earlier patch"), true);
    assert!(output.contains("written before the file's latest edits"));

    let mut pending = described(DIFF);
    pending.pending = true;
    let (output, _) = report_screen(&pending, true);
    assert!(output.contains("Replaces b"), "{output}");
    assert!(output.contains("describing each file's edits…"), "{output}");

    // A file the report is missing because describing it failed says why.
    let mut failed = described(DIFF);
    let report = failed.report.as_mut().unwrap();
    report.files.clear();
    report.failures = vec!["the errand timed out\nafter 180s".into()];
    let (output, _) = report_screen(&failed, true);
    assert!(
        output.contains("describing this file failed: the errand timed out · E: try again"),
        "{output}"
    );

    let mut hidden = described(DIFF);
    hidden.hidden = true;
    let (output, rows) = report_screen(&hidden, true);
    assert!(!output.contains("Replaces b"), "{output}");
    assert!(!output.contains("e: show edit report"), "{output}");
    assert_eq!(rows, without);

    let (output, _) = report_screen(&described(DIFF), false);
    assert!(!output.contains("Replaces b"), "only per file: {output}");
    assert!(!output.contains("f: see it per file"), "{output}");
}

/// The report page lists every file with its description, in the diff's
/// order, over whichever view is open; it says which files the report
/// leaves out or wrote about before their latest edits.
#[test]
fn the_report_page_describes_every_file_on_one_page() {
    let text = format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old\n+new\n");
    let page = |report: Option<&ReportState>, text: &str| {
        let (width, height) = (100, 24);
        let buffer = draw_frame(width, height, |frame| {
            let mut view = CheckoutDiffView::unread(Block::bordered());
            view.diff = Ok(text);
            view.report = report;
            view.report_page = Some(ReportPage {
                selected: 1,
                scroll: 0,
                reveal: true,
                preview: None,
            });
            render(frame, &view, frame.area());
        });
        buffer_rows(&buffer).join("\n")
    };
    let report = ReportState {
        report: Some(crate::Report {
            files: [(
                "src/a.rs".to_owned(),
                crate::FileReport {
                    fingerprint: crate::report::fingerprint("an earlier patch"),
                    description: "**Replaces** `b` with `c`.".into(),
                },
            )]
            .into(),
            failures: Vec::new(),
        }),
        // The page shows what the per-file view has put away.
        hidden: true,
        ..Default::default()
    };
    let output = page(Some(&report), &text);
    assert!(output.contains("Edit report"), "{output}");
    assert!(output.contains("1 of 2 files described"), "{output}");
    let a = output.find("src/a.rs [M]").unwrap();
    let described = output.find("Replaces b with c.").unwrap();
    let stale = output
        .find("written before the file's latest edits")
        .unwrap();
    let b = output.find("b.txt [M]").unwrap();
    let missing = output.find("not described").unwrap();
    assert!(a < described && described < stale && stale < b && b < missing);
    assert!(!output.contains("fn c()"), "drawn over the diff: {output}");
    assert!(output.contains("▌ b.txt [M]"), "selected: {output}");
    assert!(output.contains("▌  not described"), "all of it: {output}");
    assert!(!output.contains("▌ src/a.rs"), "{output}");

    let output = page(Some(&ReportState::default()), &text);
    assert!(
        output.contains("no edit report yet · E: write one"),
        "{output}"
    );
    assert!(!output.contains("b.txt"), "{output}");

    let output = page(None, &text);
    assert!(output.contains("no edit report to write"), "{output}");
}

/// The report page keeps the selected file in view as the selection
/// moves, and when scrolled freely moves the selection to the nearest
/// file still in view. File names stand out from the descriptions, the
/// selected one most.
#[test]
fn the_report_page_keeps_its_selection_in_view() {
    let text: String = (0..20)
        .map(|n| format!("diff --git a/f{n:02} b/f{n:02}\n@@ -1 +1 @@\n-a\n+b\n"))
        .collect();
    let report = ReportState {
        report: Some(crate::Report::default()),
        ..Default::default()
    };
    // No host frame; 10 rows inside the page's border: the summary, then three rows a file —
    // the rule above it, its name, and that it is not described.
    let draw = |page: ReportPage| {
        let mut measured = None;
        let buffer = draw_frame(60, 12, |frame| {
            let mut view = CheckoutDiffView::unread(Block::default());
            view.diff = Ok(&text);
            view.report = Some(&report);
            view.report_page = Some(page);
            measured = render(frame, &view, frame.area()).report_page;
        });
        (buffer, measured.unwrap())
    };
    let row_of = |buffer: &ratatui::buffer::Buffer, name: &str| {
        (0..12).find(|&y| {
            let row: String = (0..60).map(|x| buffer[(x, y)].symbol()).collect();
            row.contains(name)
        })
    };
    let selected = |selected, scroll, reveal| ReportPage {
        selected,
        scroll,
        reveal,
        preview: None,
    };

    let (buffer, page) = draw(selected(5, 0, true));
    assert_eq!(page, selected(5, 9, false), "f05's last row at the bottom");
    let y = row_of(&buffer, "▌ f05").unwrap();
    assert_eq!(buffer[(3, y)].fg, theme::SELECTED_FILE_NAME);
    // The band and the bar go down the whole entry, its description too,
    // and no further.
    for row in [y, y + 1] {
        assert_eq!(buffer[(1, row)].symbol(), ENTRY_MARKER);
        assert_eq!(buffer[(1, row)].fg, theme::ACCENT);
        assert_eq!(buffer[(30, row)].bg, theme::SELECTION_BACKGROUND);
    }
    assert_ne!(buffer[(30, y - 1)].bg, theme::SELECTION_BACKGROUND);
    let other = row_of(&buffer, "f04").unwrap();
    assert_eq!(buffer[(3, other)].fg, theme::DIRECTORY_NAME);
    assert_eq!(buffer[(4, other + 1)].fg, theme::MUTED_TEXT);
    assert_ne!(buffer[(30, other)].bg, theme::SELECTION_BACKGROUND);

    let (_, page) = draw(selected(3, 9, true));
    assert_eq!(page.scroll, 9, "already in view");
    let (_, page) = draw(selected(1, 9, true));
    assert_eq!(page.scroll, 4, "up to the rule above it");
    let (_, page) = draw(selected(0, 9, true));
    assert_eq!(page.scroll, 0, "and the summary above the first");
    let (_, page) = draw(selected(19, 0, true));
    assert_eq!(page, selected(19, 51, false), "the last file at the bottom");

    let (_, page) = draw(selected(0, 30, false));
    assert_eq!(page, selected(9, 30, false), "the first file still in view");
    let (_, page) = draw(selected(19, 3, false));
    assert_eq!(page, selected(3, 3, false), "the last file in view");
    let (_, page) = draw(selected(19, u16::MAX, false));
    assert_eq!(page, selected(19, 51, false), "held to the end");
}

/// With its preview on, the report page takes the top half and the
/// selected file's patch the bottom, held to what it can scroll.
#[test]
fn the_report_page_shows_the_selected_files_patch_below() {
    let text = format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old\n+new\n");
    let report = ReportState {
        report: Some(crate::Report::default()),
        ..Default::default()
    };
    let draw = |selected, preview| {
        let mut measured = None;
        let buffer = draw_frame(80, 30, |frame| {
            let mut view = CheckoutDiffView::unread(Block::bordered());
            view.diff = Ok(&text);
            view.report = Some(&report);
            view.report_page = Some(ReportPage {
                selected,
                scroll: 0,
                reveal: true,
                preview,
            });
            measured = render(frame, &view, frame.area()).report_page;
        });
        let rows = buffer_rows(&buffer);
        (rows, measured.unwrap().preview)
    };
    let row = |rows: &[String], text: &str| rows.iter().position(|row| row.contains(text));

    let (rows, preview) = draw(0, None);
    assert_eq!(preview, None);
    assert_eq!(row(&rows, "J/K: scroll · p: hide"), None, "{rows:#?}");

    let (rows, preview) = draw(0, Some(u16::MAX));
    assert_eq!(preview, Some(0), "a short patch does not scroll");
    assert!(row(&rows, "Edit report").unwrap() < 15);
    assert_eq!(row(&rows, "J/K: scroll · p: hide"), Some(15), "{rows:#?}");
    let added = row(&rows, "fn c()").unwrap();
    assert!(added > 15, "{rows:#?}");
    assert_eq!(row(&rows, "new"), None, "only the selected file's");

    assert!(rows[15].contains("Diff · src/a.rs [M] · J/K"), "{rows:#?}");

    let (rows, _) = draw(1, Some(0));
    assert!(rows[15].contains("Diff · b.txt [M] · J/K"), "{rows:#?}");
    assert!(row(&rows, "new").unwrap() > 15, "{rows:#?}");
    assert_eq!(row(&rows, "fn c()"), None);
}

/// The host's frame stays as the host drew it — focus outline and all —
/// under the pages laid over the view, which take only what is inside it.
#[test]
fn pages_over_the_view_leave_the_hosts_frame() {
    let outline = Style::default().fg(Color::Magenta);
    let report = ReportState::default();
    let pages: [fn(&mut CheckoutDiffView); 3] = [
        |view| view.help = Some(0),
        |view| view.report_page = Some(ReportPage::default()),
        |view| {
            view.report_page = Some(ReportPage {
                preview: Some(0),
                ..ReportPage::default()
            })
        },
    ];
    for page in pages {
        let (width, height) = (60, 20);
        let buffer = draw_frame(width, height, |frame| {
            let mut view = CheckoutDiffView::unread(Block::bordered().border_style(outline));
            view.diff = Ok(DIFF);
            view.report = Some(&report);
            page(&mut view);
            render(frame, &view, frame.area());
        });
        let frame: Vec<_> = (0..width)
            .flat_map(|x| [(x, 0), (x, height - 1)])
            .chain((0..height).flat_map(|y| [(0, y), (width - 1, y)]))
            .collect();
        for (x, y) in frame {
            assert_eq!(buffer[(x, y)].fg, Color::Magenta, "({x}, {y}): {buffer:?}");
        }
    }
}

/// A report embedded by a host must respect a pane with a nonzero origin.
#[test]
fn a_report_page_stays_inside_the_supplied_side_pane() {
    let mut terminal = Terminal::new(TestBackend::new(160, 30)).unwrap();
    let report = ReportState {
        report: Some(crate::Report::default()),
        ..Default::default()
    };
    terminal
        .draw(|frame| {
            frame.render_widget(
                Paragraph::new("conversation beside the diff"),
                Rect::new(0, 0, 80, 29),
            );
            let before = frame.buffer_mut().clone();
            let mut view = CheckoutDiffView::unread(Block::bordered().title("diff"));
            view.diff = Ok(DIFF);
            view.report = Some(&report);
            view.report_page = Some(ReportPage::default());
            render(frame, &view, Rect::new(80, 0, 80, 29));
            for y in 0..30 {
                for x in 0..80 {
                    assert_eq!(frame.buffer_mut()[(x, y)], before[(x, y)], "({x}, {y})");
                }
            }
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let right: String = (0..29)
        .flat_map(|y| (80..160).map(move |x| (x, y)))
        .map(|position| buffer[position].symbol())
        .collect();
    assert!(right.contains("Edit report"));
}

#[test]
fn file_search_filters_names_and_the_selected_patch_together() {
    let text = format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old second\n+new second\n");
    let files = filtered_file_diffs(&text, Some("B.T"));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].0, 1);
    assert_eq!(files[0].1.path, "b.txt");
    let ((output, _), _) = filtered_screen(
        Ok(&text),
        None,
        0,
        true,
        1,
        SearchView {
            query: Some("B.T"),
            typing: true,
        },
    );
    assert!(output.contains("/B.T"), "{output}");
    assert!(output.contains("Enter: keep filter"), "{output}");
    assert!(output.contains("› b.txt"), "{output}");
    assert!(output.contains("new second"), "{output}");
    assert!(!output.contains("src/a.rs"), "{output}");
    assert!(!output.contains("fn c()"), "{output}");

    let ((output, limit), _) = filtered_screen(
        Ok(&text),
        None,
        0,
        true,
        1,
        SearchView {
            query: Some("missing"),
            typing: false,
        },
    );
    assert!(output.contains("no matching files"), "{output}");
    assert!(!output.contains("new second"), "{output}");
    assert!(!output.contains("fn c()"), "{output}");
    assert_eq!(limit, 0);
}

#[test]
fn per_file_view_keeps_files_on_the_left_and_only_the_selected_patch_on_the_right() {
    let text = format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old second\n+new second\n");
    let ((output, _), viewport) = file_screen(Ok(&text), Some("main"), 0, true, 1);
    let rows = output.lines().collect::<Vec<_>>();
    // The panel border, the widest row ("2 files: 2 modified  +3 -2", 26),
    // padding and tree border put the patch at column 30.
    let tree = |row: &&str| row.chars().take(30).collect::<String>();
    let patch = |row: &&str| row.chars().skip(30).collect::<String>();
    for name in ["▾ src/", "  a.rs [M] +2 -1", "› b.txt [M] +1 -1"] {
        assert!(rows.iter().any(|row| tree(row).contains(name)), "{output}");
    }
    assert!(
        rows.iter()
            .any(|row| tree(row).contains("› b.txt") && tree(row).ends_with("  │")),
        "{output}"
    );
    assert!(
        rows.iter().any(|row| patch(row).contains("new second")),
        "{output}"
    );
    assert!(!output.contains("fn c()"), "{output}");
    let stat = rows
        .iter()
        .position(|row| tree(row).contains("2 files: 2 modified  +3 -2"))
        .expect(&output);
    assert!(
        tree(&rows[stat + 1]).contains("› b.txt"),
        "atop the file list: {output}"
    );
    assert_eq!(
        rows.iter().filter(|row| row.contains("2 files:")).count(),
        1,
        "not in the header too: {output}"
    );
    assert_eq!(viewport, 12, "shortcut rows leave room for the patch");
}

#[test]
fn hidden_file_list_gives_the_patch_the_full_body_in_both_positions() {
    let text = format!("{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n-old second\n+new second\n");
    for files_below in [false, true] {
        let buffer = draw_frame(80, 18, |frame| {
            let mut view = CheckoutDiffView::unread(Block::bordered());
            view.diff = Ok(&text);
            view.per_file = true;
            view.selected_file = 1;
            view.files_below = files_below;
            view.file_list_hidden = true;
            render(frame, &view, frame.area());
        });
        let rows = buffer_rows(&buffer);
        let output = rows.join("\n");
        assert!(output.contains("b.txt [M]"), "{output}");
        assert!(output.contains("new second"), "{output}");
        assert!(!output.contains("2 files:"), "{output}");
        assert!(!output.contains("▾ src/"), "{output}");
        assert!(
            rows.iter().any(|row| row.starts_with("│ b.txt [M]")),
            "{output}"
        );
    }
}

/// Below the patch, the files take the full width and no more rows than
/// FILE_LIST_MAX_HEIGHT, scrolling to keep the selection in sight.
#[test]
fn files_below_the_patch_fill_a_strip_of_bounded_height() {
    let text = (0..20)
        .map(|n| format!("diff --git a/f{n:02} b/f{n:02}\n@@ -1 +1 @@\n-old {n}\n+new {n}\n"))
        .collect::<String>();
    let (width, height) = (50, 40);
    let buffer = draw_frame(width, height, |frame| {
        let mut view = CheckoutDiffView::unread(Block::bordered());
        view.diff = Ok(&text);
        view.per_file = true;
        view.files_below = true;
        view.thumbnail = true;
        view.selected_file = 19;
        render(frame, &view, frame.area());
    });
    let output = buffer_rows(&buffer).join("\n");
    let rows = output.lines().collect::<Vec<_>>();
    let stat = rows
        .iter()
        .position(|row| row.contains("20 files: 20 modified"))
        .expect(&output);
    // The panel's bottom border closes the strip.
    assert_eq!(
        stat + usize::from(FILE_LIST_MAX_HEIGHT),
        rows.len() - 1,
        "{output}"
    );
    assert!(rows[stat - 1].contains("──────"), "a rule above: {output}");
    assert!(
        rows[stat + 1..].iter().any(|row| row.contains("› f19")),
        "{output}"
    );
    assert!(
        !rows[stat + 1..].iter().any(|row| row.contains("f00")),
        "{output}"
    );
    // The patch has the whole width above it.
    assert!(
        rows[..stat].iter().any(|row| row.contains("new 19")),
        "{output}"
    );
    assert!(
        !rows[..stat].iter().any(|row| row.contains("f18")),
        "{output}"
    );
}

#[test]
fn a_new_files_mode_line_is_not_drawn() {
    let text = "diff --git a/a b/a\nnew file mode 100644\nindex 0000000..e69de29\n\
            --- /dev/null\n+++ b/a\n@@ -0,0 +1 @@\n+one\n";
    let drawn: Vec<_> = rows(text, Hidden::default(), false)
        .into_iter()
        .filter_map(|row| match row {
            Row::Line(index) => text.lines().nth(index),
            Row::RuleAbove | Row::RuleBelow => None,
        })
        .collect();
    assert_eq!(drawn, ["diff --git a/a b/a", "@@ -0,0 +1 @@", "+one"]);
    let shown = patch_lines(
        text,
        Hidden::default(),
        Tint::Full,
        true,
        None,
        40,
        false,
        None,
    );
    assert_eq!(shown[0].to_string().trim_end(), "a mode 100644");
    let file = &crate::files::file_diffs(text)[0];
    assert_eq!(file_info(file).to_string(), " a [A] mode 100644");
}

/// Ultra-compact draws only the changed lines, under their file's header
/// and hunk cuts, numbered as they are with their context.
#[test]
fn ultra_compact_draws_only_the_changed_lines() {
    let text = "diff --git a/a b/a\nindex 1111111..2222222 100644\n--- a/a\n+++ b/a\n\
            @@ -10,3 +10,3 @@\n keep\n-old\n+new\n after\n\\ No newline at end of file\n\
            @@ -50 +50 @@\n-last\n+end\n";
    let hidden = Hidden {
        removed: false,
        context: true,
    };
    let drawn: Vec<_> = rows(text, hidden, false)
        .into_iter()
        .filter_map(|row| match row {
            Row::Line(index) => text.lines().nth(index),
            Row::RuleAbove | Row::RuleBelow => None,
        })
        .collect();
    assert_eq!(
        drawn,
        [
            "diff --git a/a b/a",
            "@@ -10,3 +10,3 @@",
            "-old",
            "+new",
            "@@ -50 +50 @@",
            "-last",
            "+end"
        ]
    );
    let shown: Vec<_> = patch_lines(text, hidden, Tint::Full, false, None, 40, false, None)
        .iter()
        .map(|line| line.to_string().trim_end().to_owned())
        .collect();
    assert!(
        shown.iter().any(|line| line.starts_with("11 -old")),
        "{shown:?}"
    );
    assert!(
        shown.iter().any(|line| line.starts_with("11 +new")),
        "{shown:?}"
    );
    assert!(!shown.iter().any(|line| line.contains("keep")), "{shown:?}");
    assert!(
        !shown.iter().any(|line| line.contains("after")),
        "{shown:?}"
    );
    assert!(
        !shown.iter().any(|line| line.contains("No newline")),
        "{shown:?}"
    );
}

#[test]
fn file_list_and_title_use_the_same_change_markers() {
    let text = "diff --git a/a b/a\nnew file mode 100644\n\
            diff --git a/b b/b\ndeleted file mode 100644\n\
            diff --git a/old b/c\nsimilarity index 100%\nrename from old\nrename to c\n\
            diff --git a/old2 b/d\nsimilarity index 90%\nrename from old2\nrename to d\n@@ -1 +1 @@\n-old\n+new\n";
    let ((output, _), _) = file_screen(Ok(text), Some("main"), 0, true, 0);
    for label in ["a [A]", "b [D]", "c [R]", "d [R+]"] {
        assert!(output.contains(label), "{output}");
    }
    assert!(output.contains("│ a [A]"), "{output}");
    let ((output, _), _) = file_screen(Ok(text), Some("main"), u16::MAX, true, 3);
    assert!(output.contains("│ d [R+]"), "{output}");
    assert!(!output.contains("│ a [A]"), "{output}");
    let ((output, _), _) = filtered_screen(
        Ok(text),
        Some("main"),
        0,
        true,
        0,
        SearchView {
            query: Some("c"),
            ..Default::default()
        },
    );
    assert!(output.contains("│ c [R]"), "{output}");
    let files = filtered_file_diffs(text, Some("c"));
    let rows = file_tree_rows(&files, true);
    assert_eq!(rows[0].0, Some(2));
    assert_eq!(rows[0].1.to_string(), "  c [R]");
    assert_eq!(rows[0].1.spans[2].style.fg, Some(theme::DIRECTORY_NAME));
    for (_, file) in filtered_file_diffs(text, None) {
        let tree = file_tree_rows(
            &[(
                0,
                FileDiff {
                    path: file.path,
                    patch: file.patch,
                },
            )],
            true,
        );
        let title = file_info(&file);
        assert_eq!(title.spans[1], tree[0].1.spans[2]);
    }
}

/// The tree fits its longest row, with a little room after it, but a
/// deep path cannot take more than half the body from the patch.
#[test]
fn the_file_tree_is_as_wide_as_its_longest_row() {
    let rows = |paths: &[&str]| {
        let text = paths
            .iter()
            .map(|path| format!("diff --git a/{path} b/{path}\n"))
            .collect::<String>();
        let files = filtered_file_diffs(&text, None);
        file_tree_rows(&files, true)
            .into_iter()
            .map(|(index, line)| (index, Line::from(line.to_string())))
            .collect::<Vec<_>>()
    };
    // "  main.rs [M]", its indent room for the marker: 13, + padding 2
    // + border 1.
    assert_eq!(file_tree_width(&rows(&["src/main.rs", "a.rs"]), 120), 16);
    let deep = ["a/very/deep/directory/structure/with/a_long_file_name.rs"];
    assert_eq!(file_tree_width(&rows(&deep), 60), 30);
    assert_eq!(file_tree_width(&[], 60), 3);
}

#[test]
fn selection_remains_visible_in_a_long_file_tree() {
    let text = (0..30)
            .map(|n| format!("diff --git a/src/dir{n}/file{n} b/src/dir{n}/file{n}\n@@ -1 +1 @@\n-old{n}\n+new{n}\n"))
            .collect::<String>();
    let ((output, _), _) = file_screen(Ok(&text), None, 0, true, 29);
    assert!(output.contains("  ▾ dir29/"), "{output}");
    assert!(output.contains("  › file29"), "{output}");
    assert!(output.contains("new29"), "{output}");
    assert!(!output.contains("new0"), "{output}");
    // Directories are never selected, so the top one needs no room for
    // the marker.
    let ((output, _), _) = file_screen(Ok(&text), None, 0, true, 0);
    assert!(output.contains("│▾ src/"), "{output}");
    assert!(output.contains("│  ▾ dir0/"), "{output}");
    assert!(output.contains("│  › file0"), "{output}");
}

#[test]
fn the_selected_file_sits_on_a_soft_band_and_keeps_its_markers_color() {
    let text = "diff --git a/a b/a\nnew file mode 100644\n\
            diff --git a/b b/b\ndeleted file mode 100644\n";
    let buffer = draw_frame(40, 12, |frame| {
        render(
            frame,
            &CheckoutDiffView {
                per_file: true,
                diff: Ok(text),
                selected_file: 1,
                ..CheckoutDiffView::unread(Block::bordered())
            },
            frame.area(),
        );
    });
    let row = |name: &str| {
        (0..12)
            .find(|&y| {
                (0..40)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .contains(name)
            })
            .unwrap()
    };
    let (selected, other) = (row("› b [D]"), row("  a [A]"));
    // The marker, the name, and the change marker's bracket.
    for x in [1, 3, 5] {
        assert_eq!(buffer[(x, selected)].bg, theme::SELECTION_BACKGROUND);
        assert_ne!(buffer[(x, other)].bg, theme::SELECTION_BACKGROUND);
    }
    assert_eq!(buffer[(3, selected)].fg, theme::SELECTED_FILE_NAME);
    assert_ne!(buffer[(3, other)].fg, theme::SELECTED_FILE_NAME);
    assert_eq!(buffer[(6, selected)].fg, theme::ERROR);
}

#[test]
fn file_tree_scrolls_with_five_rows_of_margin_in_both_directions() {
    let text = (0..60)
        .map(|n| format!("diff --git a/src/file{n:02} b/src/file{n:02}\n"))
        .collect::<String>();
    let mut terminal = Terminal::new(TestBackend::new(80, 26)).unwrap();
    let mut offset = 0;
    // The directory takes the first row; the file's row is its index + 1.
    for (selected_file, expected_offset) in [
        (0, 0),
        (13, 0),
        (14, 1),
        (15, 2),
        (30, 17),
        (22, 17),
        (21, 17),
        (20, 16),
        (0, 0),
        (59, 41),
    ] {
        terminal
            .draw(|frame| {
                let measured = render(
                    frame,
                    &CheckoutDiffView {
                        worktree: Path::new("/w"),
                        base_commit: "4bf5c35d",
                        branch_point: true,
                        diff: Ok(&text),
                        per_file: true,
                        selected_file,
                        file_list_offset: offset,
                        ..CheckoutDiffView::unread(Block::bordered())
                    },
                    frame.area(),
                );
                assert_eq!(measured.scroll.viewport, 20);
                offset = measured.file_list_offset;
            })
            .unwrap();
        assert_eq!(offset, expected_offset, "selected file {selected_file}");
        let selection_row = (0..26)
            .find(|&y| terminal.backend().buffer()[(1, y)].symbol() == "›")
            .expect("selection remains visible");
        if (14..=30).contains(&selected_file) {
            // The body spans rows 5 through 24, inclusive.
            assert!(selection_row >= 5 + 5);
            assert!(selection_row <= 24 - 5);
        }
    }
}

#[test]
fn tree_groups_shared_directories_and_keeps_original_patch_indices_when_filtered() {
    let text = [
        "src/a.rs",
        "src/a/z.rs",
        "README.md",
        "src/a/b.rs",
        "src/z.rs",
    ]
    .iter()
    .map(|path| format!("diff --git a/{path} b/{path}\n"))
    .collect::<String>();
    let files = filtered_file_diffs(&text, None);
    let rows = file_tree_rows(&files, false);
    assert_eq!(
        rows.len(),
        file_tree_rows(&files, true).len(),
        "nothing to fold"
    );
    let displayed: Vec<_> = rows
        .iter()
        .map(|(index, line)| (*index, line.to_string()))
        .collect();
    assert_eq!(
        displayed,
        vec![
            (Some(2), "  README.md [M]".into()),
            (None, "▾ src/".into()),
            (None, "  ▾ a/".into()),
            (Some(3), "    b.rs [M]".into()),
            (Some(1), "    z.rs [M]".into()),
            (Some(0), "  a.rs [M]".into()),
            (Some(4), "  z.rs [M]".into()),
        ]
    );
    let files = filtered_file_diffs(&text, Some("src/a/"));
    assert_eq!(
        files.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
        [3, 1]
    );
    assert_eq!(file_tree_rows(&files, false).len(), 4);
    assert_eq!(file_tree_rows(&files, true).len(), 3, "src/ holds only a/");
}

/// Directories with no files of their own and one subdirectory fold onto
/// a single row; one with several subdirectories keeps its own. Folded or
/// not, directories are told from files by colour.
#[test]
fn directories_without_files_fold_into_one_row() {
    let text = [
        "styra/diff/src/lib.rs",
        "styra/diff/src/view.rs",
        "styra/ui/src/scroll.rs",
        "styra/ui/src/search.rs",
        "Cargo.toml",
    ]
    .iter()
    .map(|path| format!("diff --git a/{path} b/{path}\n"))
    .collect::<String>();
    let files = filtered_file_diffs(&text, None);
    let shown = |compact| {
        file_tree_rows(&files, compact)
            .iter()
            .map(|(_, line)| line.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        shown(true),
        [
            "  Cargo.toml [M]",
            "▾ styra/",
            "  ▾ diff/src/",
            "    lib.rs [M]",
            "    view.rs [M]",
            "  ▾ ui/src/",
            "    scroll.rs [M]",
            "    search.rs [M]",
        ]
    );
    assert_eq!(
        shown(false),
        [
            "  Cargo.toml [M]",
            "▾ styra/",
            "  ▾ diff/",
            "    ▾ src/",
            "      lib.rs [M]",
            "      view.rs [M]",
            "  ▾ ui/",
            "    ▾ src/",
            "      scroll.rs [M]",
            "      search.rs [M]",
        ]
    );
    for (index, line) in file_tree_rows(&files, true) {
        let directory = line.spans[0].style.fg == Some(theme::DIRECTORY_NAME);
        assert_eq!(directory, index.is_none(), "{line}");
    }
}

#[test]
fn checkout_headers_describe_the_base_and_its_names() {
    let names = [
        "main".to_owned(),
        "styra/rename".into(),
        "tag: v1.2".into(),
        "origin/main".into(),
    ];
    let checkout = |base_branch| CheckoutDiffView {
        worktree: Path::new("/state/worktrees/styra-7"),
        branch: Some("styra/rename"),
        base_branch,
        base_commit: "4bf5c35d0123456789abcdef",
        branch_point: true,
        diff: Ok(DIFF),
        ..CheckoutDiffView::unread(Block::bordered())
    };
    for (case, width, height, view, present, absent) in [
            (
                "branch point",
                80,
                18,
                checkout(Some("main")),
                &[
                    "styra/rename  against  main at 4bf5c35d01 (no branch tip or tag)  (where",
                    "/state/worktrees/styra-7",
                    "1 file: 1 modified  +2 -1",
                    "fn c() {}",
                ][..],
                &[][..],
            ),
            (
                "detached base",
                80,
                18,
                checkout(None),
                &["styra/rename  against  4bf5c35d01"][..],
                &[][..],
            ),
            (
                "comparison without a branch point",
                80,
                16,
                CheckoutDiffView {
                    branch: Some("main"),
                    base_branch: Some("HEAD"),
                    base_commit: "4bf5c35d0123456789abcdef",
                    diff: Ok(""),
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                &[
                    "main  against  HEAD at 4bf5c35d01",
                    "no changes",
                    "no branch tip or tag",
                ][..],
                &["where the branch started", "since the branch was made"][..],
            ),
            (
                "branches and tags at the base",
                180,
                16,
                CheckoutDiffView {
                    base_names: &names,
                    ..checkout(Some("main"))
                },
                &[
                    "against  main at 4bf5c35d01 (branch tip: main, branch tip: styra/rename, tag: v1.2, branch tip: origin/main)  (where the branch started)",
                ][..],
                &[][..],
            ),
        ] {
            let buffer = draw_frame(width, height, |frame| {
                render(frame, &view, frame.area());
            });
            let output: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
            for text in present {
                assert!(output.contains(text), "{case}: missing {text:?}: {output}");
            }
            for text in absent {
                assert!(!output.contains(text), "{case}: unexpected {text:?}: {output}");
            }
        }
}

/// A host's conversation takes the header's place; the diff stays.
#[test]
fn a_conversation_replaces_the_header() {
    let buffer = draw_frame(80, 16, |frame| {
        render(
            frame,
            &CheckoutDiffView {
                branch: Some("styra/rename"),
                base_commit: "4bf5c35d0123456789abcdef",
                diff: Ok(DIFF),
                conversation: Some(vec![Line::from("» rename b to c")]),
                ..CheckoutDiffView::unread(Block::bordered())
            },
            frame.area(),
        );
    });
    let output = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(output.contains("» rename b to c"), "{output}");
    assert!(!output.contains("against"), "{output}");
    assert!(!output.contains("1 file:"), "{output}");
    assert!(output.contains("fn c() {}"), "{output}");
}

/// A supplied diff is called what the host says it is, rather than a
/// comparison of branches it has none of, and its sides are before and
/// after rather than a base commit and the working tree.
#[test]
fn a_supplied_diff_is_labelled_as_given() {
    let draw = |side_by_side, diff| {
        let buffer = draw_frame(100, 16, |frame| {
            render(
                frame,
                &CheckoutDiffView {
                    label: Some("turn 3 · snapshot diff"),
                    diff: Ok(diff),
                    side_by_side,
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
        });
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    let output = draw(false, DIFF);
    assert!(output.contains("turn 3 · snapshot diff"), "{output}");
    assert!(!output.contains("against"), "{output}");
    assert!(!output.contains("working tree"), "{output}");
    assert!(output.contains("1 file: 1 modified"), "{output}");
    let output = draw(true, DIFF);
    assert!(output.contains("After"), "{output}");
    assert!(output.contains("│ Before"), "{output}");
    assert!(!output.contains("Base ·"), "{output}");
    let output = draw(false, "");
    assert!(output.contains("no changes"), "{output}");
    assert!(!output.contains("since the branch"), "{output}");
}

/// A hunk without a position is drawn side by side without numbers
/// rather than counted from 0.
#[test]
fn a_hunk_without_a_position_has_no_numbers_side_by_side() {
    let buffer = draw_frame(120, 16, |frame| {
        render(
                frame,
                &CheckoutDiffView {
                    label: Some("turn 1"),
                    diff: Ok("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ edit @@\n-fn old_name\n+fn new_name\n"),
                    side_by_side: true,
                    rendered_side: true,
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
    });
    let output = buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(output.contains("-fn old_name"), "{output}");
    assert!(!output.contains("0 -fn old_name"), "{output}");
    assert!(!output.contains("0 +fn new_name"), "{output}");
}

/// A host's intro sits above the header, which stays, and is held to
/// half the panel however long it is.
#[test]
fn an_intro_goes_above_the_header_and_takes_at_most_half() {
    let draw = |intro: Vec<Line<'static>>| {
        let buffer = draw_frame(80, 20, |frame| {
            render(
                frame,
                &CheckoutDiffView {
                    label: Some("turn 2"),
                    diff: Ok(DIFF),
                    intro: Some(intro),
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
        });
        buffer_rows(&buffer)
    };
    let rows = draw(vec![Line::from("rename b to c")]);
    let row = |rows: &[String], text: &str| rows.iter().position(|row| row.contains(text));
    assert!(row(&rows, "rename b to c").unwrap() < row(&rows, "turn 2").unwrap());
    let rows = draw((0..40).map(|i| Line::from(format!("line {i}"))).collect());
    assert!(row(&rows, "line 0").is_some());
    assert!(row(&rows, "line 20").is_none(), "{rows:#?}");
    assert!(row(&rows, "turn 2").is_some(), "{rows:#?}");
}

/// Each file's header is a bar between two rules drawn as rows of their
/// own: one along its bottom, and — for every file after the first — one
/// along its top. Hidden removed lines take no rule with them.
#[test]
fn file_headers_are_bars_between_rules() {
    let text = format!(
        "{DIFF}diff --git a/b.txt b/b.txt\n@@ -1 +1 @@\n+kept\n-gone\n\
             diff --git a/c.txt b/c.txt\n@@ -1 +1 @@\n-x\n+y\n"
    );
    // The rows beneath the view's own header: a bar in brackets, a rule
    // as its glyph alone, and any other row as its text.
    let body = |hide_removed: bool| {
        let height = 40;
        let buffer = draw_frame(80, height, |frame| {
            render(
                frame,
                &CheckoutDiffView {
                    diff: Ok(&text),
                    hide_removed,
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
        });
        let rows: Vec<(String, bool, bool)> = (0..height)
            .map(|y| {
                let cells: Vec<_> = (1..79u16).map(|x| &buffer[(x, y)]).collect();
                let text: String = cells.iter().map(|cell| cell.symbol()).collect();
                let bar = cells
                    .iter()
                    .all(|cell| cell.bg == theme::DIFF_FILE_HEADER_BACKGROUND);
                let rule = cells.iter().all(|cell| cell.fg == theme::DIFF_FILE_RULE);
                (text, bar, rule)
            })
            .collect();
        // Inside the panel's own border, past the rule under the header.
        let inside = &rows[1..rows.len() - 1];
        let start = inside
            .iter()
            .position(|(text, ..)| text.starts_with("──"))
            .unwrap();
        inside[start + 1..]
            .iter()
            .map(|(text, bar, rule)| match (bar, rule) {
                (true, _) => format!("[{}]", text.trim()),
                (_, true) => text.chars().next().unwrap().to_string(),
                _ => text.trim().to_owned(),
            })
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>()
    };
    assert!(
        body(false).iter().all(|row| {
            row.chars().all(|glyph| glyph != '▁' && glyph != '▔') || row.chars().count() == 1
        }),
        "rules run edge to edge"
    );
    // Each hunk's cut runs the width of the panel.
    let cut = "┄".repeat(78);
    let cut = cut.as_str();
    assert_eq!(
        body(false),
        [
            "[src/a.rs]",
            "▔",
            cut,
            "1  fn a() {}",
            "2 -fn b() {}",
            "2 +fn c() {}",
            "3 +fn d() {}",
            "▁",
            "[b.txt]",
            "▔",
            cut,
            "1 +kept",
            "1 -gone",
            "▁",
            "[c.txt]",
            "▔",
            cut,
            "1 -x",
            "1 +y",
        ]
    );
    let hidden = body(true);
    assert_eq!(hidden.len(), body(false).len() - 3, "{hidden:?}");
    assert_eq!(hidden[10..13], ["1 +kept", "▁", "[c.txt]"]);
}

/// `t` switches the unchanged rows between faded as everywhere else and
/// half as faded; the bands stay throughout, and hiding removed lines
/// does not depend on them.
#[test]
fn the_unchanged_rows_tint_switches_between_full_and_half() {
    let draw = |tint: Tint, hide_removed: bool| {
        let buffer = draw_frame(80, 18, |frame| {
            render(
                frame,
                &CheckoutDiffView {
                    diff: Ok(DIFF),
                    hide_removed,
                    tint,
                    ..CheckoutDiffView::unread(Block::bordered())
                },
                frame.area(),
            );
        });
        let rows: Vec<String> = buffer_rows(&buffer);
        // The `f` opening a row's code.
        let start = |code: &str| {
            let y = rows.iter().position(|row| row.contains(code))?;
            let x = rows[y][..rows[y].find(code)?].chars().count();
            Some(&buffer[(x as u16, y as u16)])
        };
        let added = rows.iter().position(|row| row.contains("fn c()")).unwrap();
        (
            start("fn a()").unwrap().fg,
            start("fn c()").unwrap().fg,
            buffer[(70, added as u16)].bg,
            start("fn b()").is_some(),
        )
    };
    let (full, added, band, removed) = draw(Tint::Full, false);
    let (half, _, half_band, _) = draw(Tint::Half, false);
    assert_ne!(full, added);
    assert!(
        ![full, added].contains(&half),
        "{full:?} {half:?} {added:?}"
    );
    for band in [band, half_band] {
        assert_eq!(band, theme::DIFF_ADDED_BACKGROUND);
    }
    assert!(removed);
    assert!(!draw(Tint::Half, true).3);
    assert_eq!(Tint::Full.next().next(), Tint::Full);
}

#[test]
fn the_diff_scrolls_beneath_a_header_that_stays() {
    let long = (0..20).fold(DIFF.to_owned(), |text, n| {
        text + &format!("+fn x{n}() {{}}\n")
    });
    let (output, limit) = screen(Ok(&long), Some("main"), u16::MAX);
    assert!(limit > 0);
    assert!(output.contains("against"), "{output}");
    assert!(output.contains("fn x19() {}"), "{output}");
    assert!(!output.contains("diff --git"), "{output}");
}

#[test]
fn stat_line_counts_each_kind_of_file_change() {
    let text = format!(
        "{DIFF}\
diff --git a/new b/new
new file mode 100644
--- /dev/null
+++ b/new
@@ -0,0 +1 @@
+new
diff --git a/gone b/gone
deleted file mode 100644
diff --git a/old b/moved
similarity index 100%
rename from old
rename to moved
diff --git a/tool b/tool
old mode 100644
new mode 100755
"
    );
    let line = stat_line(&text);
    let shown: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(
        shown,
        "5 files: 1 added, 1 removed, 1 moved, 2 modified  +3 -1"
    );
}

#[test]
fn no_changes_and_failures_say_so() {
    let (empty, limit) = screen(Ok(""), Some("main"), 0);
    assert_eq!(limit, 0);
    assert!(
        empty.contains("no changes since the branch was made"),
        "{empty}"
    );
    assert!(empty.contains("0 files changed"), "{empty}");

    let (failed, _) = screen(Err("git diff failed: bad revision"), Some("main"), 0);
    assert!(failed.contains("git diff failed: bad revision"), "{failed}");
    assert!(!failed.contains("0 files"), "{failed}");
}

#[test]
fn the_commit_lists_outline_shows_whether_the_list_has_the_keys() {
    let mut commits = crate::state::Commits {
        entries: vec![crate::git::Commit {
            id: "abcdef1234".into(),
            author: "Ada".into(),
            date: "2 days ago".into(),
            summary: "First".into(),
            body: String::new(),
        }],
        open: true,
        focused: true,
        selected: 1,
    };
    let draw = |commits: &crate::state::Commits| {
        let mut terminal = Terminal::new(TestBackend::new(60, 4)).unwrap();
        terminal
            .draw(|frame| render_commits(frame, commits, frame.area()))
            .unwrap()
            .buffer
            .clone()
    };

    // Row 0 and row 3 are the outline; row 2 is the selected commit.
    let buffer = draw(&commits);
    assert_eq!(buffer[(0, 0)].symbol(), "┏");
    assert_eq!(buffer[(0, 2)].symbol(), "┃");
    assert_eq!(buffer[(59, 3)].symbol(), "┛");
    assert_eq!(buffer[(0, 2)].fg, theme::ACCENT);
    assert_eq!(buffer[(1, 2)].symbol(), "›");
    assert_eq!(buffer[(3, 2)].fg, theme::ACCENT);
    assert_eq!(buffer[(3, 1)].fg, theme::TEXT);

    commits.focused = false;
    let buffer = draw(&commits);
    assert_eq!(buffer[(0, 0)].symbol(), "┌");
    assert_eq!(buffer[(0, 2)].symbol(), "│");
    assert_eq!(buffer[(0, 2)].fg, theme::INACTIVE);
    assert_eq!(buffer[(1, 2)].symbol(), "›");
    assert_eq!(buffer[(3, 2)].fg, theme::TEXT);
    assert_eq!(
        buffer[(3, 1)].fg,
        theme::MUTED_TEXT,
        "the rest of the list recedes"
    );
    assert_eq!(buffer[(3, 1)].symbol(), "A");
}
