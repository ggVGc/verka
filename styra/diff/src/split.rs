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

/// The row diff line `line` is drawn on: side by side, the row it shares with
/// its other half; where it is hidden, the row of the first line after it.
pub(crate) fn row_of(text: &str, hidden: bool, split: bool, per_file: bool, line: usize) -> usize {
    let source = Row::Line(line);
    let exact = if split {
        aligned(text, per_file)
            .iter()
            .position(|row| row.contains(source))
    } else {
        rows(text, hidden, per_file)
            .iter()
            .position(|row| *row == source)
    };
    exact.unwrap_or_else(|| {
        let rows = source_rows(text, hidden, split, per_file);
        rows.iter()
            .position(|row| matches!(row, Row::Line(other) if *other >= line))
            .unwrap_or(rows.len().saturating_sub(1))
    })
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
mod tests;
