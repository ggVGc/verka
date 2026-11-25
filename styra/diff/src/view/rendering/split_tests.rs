use super::*;
use crate::test_support::draw as draw_frame;

#[test]
fn turn_highlights_preserve_search_matches() {
    let patch = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-old unique\n+new unique\n";
    for split in [false, true] {
        let buffer = draw_frame(160, 24, |frame| {
            let view = CheckoutDiffView {
                diff: Ok(patch),
                highlight_diff: Some(patch),
                side_by_side: split,
                line_cursor: Some(2),
                find: SearchView {
                    query: Some("unique"),
                    typing: false,
                },
                ..CheckoutDiffView::unread(Block::default())
            };
            render(frame, &view, frame.area());
        });
        let found = (0..24)
            .find_map(|y| {
                let row: String = (0..160).map(|x| buffer[(x, y)].symbol()).collect();
                let start = row.find("new unique")?;
                let x = row[..start].chars().count() as u16 + 4;
                Some(&buffer[(x, y)])
            })
            .unwrap();
        // In split view both halves share the cursor's row, whose
        // current search match uses the inverted search style.
        assert_eq!(
            found.bg,
            if split {
                theme::CODE_BACKGROUND
            } else {
                theme::ACCENT
            }
        );
        if split {
            assert_eq!(found.fg, theme::LIGHT_ACCENT);
        }
    }
}

#[test]
fn turn_highlights_preserve_all_changes_and_stay_on_the_correct_side() {
    let patch = "diff --git a/a.rs b/a.rs\n@@ -1,2 +1,2 @@\n-old unique\n+new unique\n-other old\n+other new\n";
    let turn = "diff --git a/a.rs b/a.rs\n@@ -40 +40 @@\n-old unique\n+new unique\n";
    for split in [false, true] {
        for per_file in [false, true] {
            for hidden in [false, true] {
                for enabled in [false, true] {
                    let buffer = draw_frame(160, 32, |frame| {
                        let view = CheckoutDiffView {
                            diff: Ok(patch),
                            highlight_diff: enabled.then_some(turn),
                            side_by_side: split,
                            per_file,
                            hide_removed: hidden,
                            ..CheckoutDiffView::unread(Block::default())
                        };
                        render(frame, &view, frame.area());
                    });
                    for (code, selected) in [
                        ("new unique", true),
                        ("old unique", true),
                        ("other new", false),
                        ("other old", false),
                    ] {
                        let found = (0..32).find_map(|y| {
                            let row: String = (0..160).map(|x| buffer[(x, y)].symbol()).collect();
                            let byte = row.find(code)?;
                            let x = row[..byte].chars().count() as u16
                                + code.find(' ').unwrap() as u16
                                + 1;
                            Some(&buffer[(x, y)])
                        });
                        if hidden && code.contains("old") {
                            assert!(found.is_none());
                        } else {
                            let cell = found.unwrap_or_else(|| panic!("missing {code}"));
                            assert_eq!(
                                cell.bg == theme::DIFF_TURN_BACKGROUND,
                                enabled && selected,
                                "{code}: split={split} per_file={per_file} hidden={hidden}"
                            );
                        }
                    }
                }
            }
        }
    }
}

fn split_buffer(
    patch: &str,
    width: u16,
    height: u16,
    horizontal_scroll: u16,
) -> ratatui::buffer::Buffer {
    draw_frame(width, height, |frame| {
        render_split(
            frame,
            patch,
            Tint::Full,
            true,
            None,
            frame.area(),
            0,
            horizontal_scroll,
            Hidden::default(),
            false,
            None,
        );
    })
}

#[test]
fn split_paints_each_side_and_leaves_missing_cells_neutral() {
    let patch = "diff --git a/a.rs b/a.rs\n@@ -10,2 +20,3 @@\n-old\n+new\n+extra\n keep\n";
    let buffer = split_buffer(patch, 101, 8, 0);
    let row = |y| {
        (0..101)
            .map(|x| buffer[(x, y)].symbol())
            .collect::<String>()
    };
    assert!(row(3).starts_with("20 +new"), "{}", row(3));
    assert!(row(3).contains("│10 -old"), "{}", row(3));
    assert!(row(4).starts_with("21 +extra"));
    assert_eq!(buffer[(49, 3)].bg, theme::DIFF_ADDED_BACKGROUND);
    assert_eq!(buffer[(100, 3)].bg, theme::DIFF_REMOVED_BACKGROUND);
    assert_ne!(buffer[(100, 4)].bg, theme::DIFF_REMOVED_BACKGROUND);
    assert!(row(5).starts_with("22  keep"));
    assert!(row(5).contains("│11  keep"));
}

#[test]
fn horizontal_scroll_keeps_gutters_and_divider_fixed() {
    let patch = format!(
        "@@ -123 +456 @@\n-{}tail\n+{}tail\n",
        "x".repeat(80),
        "y".repeat(80)
    );
    let buffer = split_buffer(&patch, 101, 4, 32);
    let row: String = (0..101).map(|x| buffer[(x, 1)].symbol()).collect();
    assert!(row.starts_with("456 yyyy"), "{row}");
    assert!(row.contains("│123 xxxx"), "{row}");
    assert_eq!(buffer[(50, 1)].symbol(), "│");
}

#[test]
fn narrow_and_hidden_removal_fallbacks_keep_the_requested_layout() {
    let mut view = CheckoutDiffView::unread(Block::default());
    view.diff = Ok("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n");
    view.side_by_side = true;
    for (width, hidden, expected) in [
        (81, false, true),
        (80, false, false),
        (81, true, false),
        (81, false, true),
        (0, false, false),
    ] {
        view.hide_removed = hidden;
        draw_frame(width, 25, |frame| {
            let measured = render(frame, &view, frame.area());
            assert_eq!(measured.side_by_side, expected);
            let panel = Panel::render(&view, frame, frame.area());
            assert_eq!(panel.diff_layout.unwrap().0, expected);
        });
        assert!(view.side_by_side);
    }
}
#[test]
fn words_work_in_both_layouts_with_tint_and_removals_hidden() {
    let patch = "@@ -1 +1 @@\n-let timeout = 30;\n+let timeout = 60;\n";
    for split in [false, true] {
        for words in [false, true] {
            for tint in [Tint::Full, Tint::Half] {
                let buffer = draw_frame(160, 24, |frame| {
                    let view = CheckoutDiffView {
                        diff: Ok(patch),
                        side_by_side: split,
                        word_highlights: words,
                        tint,
                        ..CheckoutDiffView::unread(Block::default())
                    };
                    render(frame, &view, frame.area());
                });
                let cells: Vec<_> = buffer
                    .content
                    .iter()
                    .filter(|cell| cell.bg == theme::DIFF_WORD_ADDED_BACKGROUND)
                    .collect();
                assert_eq!(cells.len(), usize::from(words));
                if words {
                    assert_eq!(cells[0].symbol(), "6");
                }
            }
        }
    }
    let lines = patch_lines(
        patch,
        Hidden::removed(true),
        Tint::Half,
        true,
        None,
        80,
        false,
        None,
    );
    assert!(lines
        .iter()
        .flat_map(|l| &l.spans)
        .any(|s| s.content == "6" && s.style.bg == Some(theme::DIFF_WORD_ADDED_BACKGROUND)));
    assert!(lines
        .iter()
        .flat_map(|l| &l.spans)
        .all(|s| s.style.bg != Some(theme::DIFF_WORD_REMOVED_BACKGROUND)));
}

#[test]
fn panning_keeps_word_highlights_and_gutters_in_place() {
    let padding = "a".repeat(45);
    let patch = format!("@@ -12 +12 @@\n-let {padding} = 30;\n+let {padding} = 60;\n");
    let buffer = split_buffer(&patch, 81, 3, 30);
    let changed: Vec<_> = buffer
        .content
        .iter()
        .filter(|c| c.bg == theme::DIFF_WORD_ADDED_BACKGROUND)
        .collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].symbol(), "6");
    assert_eq!(buffer[(0, 1)].symbol(), "1");
    assert_eq!(buffer[(1, 1)].symbol(), "2");
    assert_eq!(buffer[(41, 1)].symbol(), "1");
    assert_eq!(buffer[(42, 1)].symbol(), "2");
}
