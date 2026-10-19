//! Alignment and source locations shared by drawing, navigation, and resize handling.
use crate::view::{rows, Row};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Cell {
    pub index: usize,
    pub number: Option<u32>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SplitRow {
    Shared(Row),
    Pair(Option<Cell>, Option<Cell>),
}

impl SplitRow {
    pub fn source(self) -> Row {
        match self {
            Self::Shared(row) => row,
            Self::Pair(left, right) => Row::Line(right.or(left).unwrap().index),
        }
    }
    fn contains(self, row: Row) -> bool {
        match self {
            Self::Shared(other) => row == other,
            Self::Pair(left, right) => [left, right]
                .into_iter()
                .flatten()
                .any(|cell| row == Row::Line(cell.index)),
        }
    }
}

pub(crate) fn aligned(text: &str, per_file: bool) -> Vec<SplitRow> {
    let mut result = Vec::new();
    let mut old = 0;
    let mut new = 0;
    let mut in_hunk = false;
    let mut left = Vec::new();
    let mut right = Vec::new();
    fn flush(result: &mut Vec<SplitRow>, left: &mut Vec<Cell>, right: &mut Vec<Cell>) {
        let (mut l, mut r) = (0, 0);
        while l < left.len() || r < right.len() {
            let note = left.get(l).is_some_and(|c| c.number.is_none())
                || right.get(r).is_some_and(|c| c.number.is_none());
            let a = left.get(l).copied().filter(|c| c.number.is_none() == note);
            let b = right.get(r).copied().filter(|c| c.number.is_none() == note);
            l += usize::from(a.is_some());
            r += usize::from(b.is_some());
            result.push(SplitRow::Pair(a, b));
        }
        left.clear();
        right.clear();
    }
    let lines: Vec<_> = text.lines().collect();
    for row in rows(text, false, per_file) {
        let Row::Line(index) = row else {
            flush(&mut result, &mut left, &mut right);
            result.push(SplitRow::Shared(row));
            continue;
        };
        let line = lines[index];
        if line.starts_with("@@ ") {
            let mut ranges = line[3..].split_whitespace();
            let start = |range: Option<&str>| {
                range
                    .and_then(|s| s.get(1..))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            old = start(ranges.next());
            new = start(ranges.next());
            in_hunk = true;
        } else if line.starts_with("diff --git ") {
            in_hunk = false;
        } else if in_hunk {
            let cell = |number| Cell {
                index,
                number: Some(number),
            };
            match line.as_bytes().first() {
                Some(b'-') => {
                    left.push(cell(old));
                    old += 1;
                    continue;
                }
                Some(b'+') => {
                    right.push(cell(new));
                    new += 1;
                    continue;
                }
                Some(b' ') | None => {
                    flush(&mut result, &mut left, &mut right);
                    result.push(SplitRow::Pair(Some(cell(old)), Some(cell(new))));
                    old += 1;
                    new += 1;
                    continue;
                }
                Some(b'\\') => {
                    // The annotation belongs to the preceding side, and has no line number.
                    let note = Cell {
                        index,
                        number: None,
                    };
                    if lines
                        .get(index.wrapping_sub(1))
                        .is_some_and(|s| s.starts_with('-'))
                    {
                        left.push(note);
                    } else {
                        right.push(note);
                    }
                    continue;
                }
                _ => {}
            }
        }
        flush(&mut result, &mut left, &mut right);
        result.push(SplitRow::Shared(row));
    }
    flush(&mut result, &mut left, &mut right);
    result
}

pub(crate) fn source_rows(text: &str, hidden: bool, split: bool, per_file: bool) -> Vec<Row> {
    if split {
        aligned(text, per_file)
            .into_iter()
            .map(SplitRow::source)
            .collect()
    } else {
        rows(text, hidden, per_file)
    }
}

pub(crate) fn remap(
    text: &str,
    hidden: bool,
    before: bool,
    next_hidden: bool,
    after: bool,
    top: u16,
    per_file: bool,
) -> u16 {
    if before == after && hidden == next_hidden {
        return top;
    }
    let previous = source_rows(text, hidden, before, per_file);
    let Some(mut source) = previous.get(usize::from(top)).copied() else {
        return u16::MAX;
    };
    // Rules repeat between files. Anchor them to the following source line.
    let rule = !matches!(source, Row::Line(_));
    if rule {
        source = previous
            .iter()
            .skip(usize::from(top))
            .copied()
            .find(|row| matches!(row, Row::Line(_)))
            .unwrap_or(source);
    }
    let next = source_rows(text, next_hidden, after, per_file);
    let exact = if after {
        aligned(text, per_file)
            .iter()
            .position(|row| row.contains(source))
    } else {
        next.iter().position(|row| *row == source)
    };
    let at = exact
        .or_else(|| {
            let Row::Line(index) = source else {
                return None;
            };
            next.iter()
                .position(|row| matches!(row, Row::Line(other) if *other >= index))
        })
        .unwrap_or(next.len());
    let at = if rule && at > 0 && next.get(at - 1) == previous.get(usize::from(top)) {
        at - 1
    } else {
        at
    };
    u16::try_from(at).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
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
}
