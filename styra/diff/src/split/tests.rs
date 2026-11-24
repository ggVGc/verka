use super::*;

#[test]
fn unequal_changes_align_without_crossing_context_or_hunks() {
    let text = "diff --git a/a b/a\n@@ -10,4 +20,5 @@\n keep\n-old\n+new\n+extra\n after\n@@ -50 +60 @@\n-last\n+end\n";
    let pairs: Vec<_> = aligned(text, false)
        .into_iter()
        .filter_map(|row| match row {
            SplitRow::Pair(a, b) => Some((a.map(|c| c.number), b.map(|c| c.number))),
            _ => None,
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            (Some(Some(10)), Some(Some(20))),
            (Some(Some(11)), Some(Some(21))),
            (None, Some(Some(22))),
            (Some(Some(12)), Some(Some(23))),
            (Some(Some(50)), Some(Some(60))),
        ]
    );
}

#[test]
fn annotations_do_not_pair_with_code_or_advance_numbers() {
    let text = "@@ -1 +1,2 @@\n-old\n\\ No newline at end of file\n+new\n+extra\n\\ No newline at end of file\n";
    let pairs: Vec<_> = aligned(text, false)
        .into_iter()
        .filter_map(|r| match r {
            SplitRow::Pair(a, b) => Some((a.map(|c| c.index), b.map(|c| c.index))),
            _ => None,
        })
        .collect();
    assert_eq!(
        pairs,
        vec![
            (Some(1), Some(3)),
            (Some(2), None),
            (None, Some(4)),
            (None, Some(5))
        ]
    );
}

#[test]
fn layout_switches_anchor_both_halves_and_later_file_rules() {
    let text = "diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\ndiff --git a/b b/b\n@@ -1 +1 @@\n-before\n+after\n";
    let unified = rows(text, false, false);
    let split = aligned(text, false);
    for (top, row) in unified.iter().enumerate() {
        let target = remap(text, false, false, false, true, top as u16, false);
        if let Row::Line(_) = row {
            assert!(split[usize::from(target)].contains(*row));
        } else {
            assert_eq!(split[usize::from(target)].source(), *row);
        }
    }
    let top = split
        .iter()
        .position(|row| row.contains(Row::Line(2)))
        .unwrap();
    let target = remap(text, false, true, false, false, top as u16, false);
    assert_eq!(unified[usize::from(target)], Row::Line(3));
}

#[test]
fn metadata_only_files_remain_full_width() {
    let text = "diff --git a/a b/b\nsimilarity index 100%\nrename from a\nrename to b\ndiff --git a/c b/c\nBinary files a/c and b/c differ\n";
    assert!(aligned(text, false)
        .iter()
        .all(|row| matches!(row, SplitRow::Shared(_))));
}
