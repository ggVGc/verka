//! Match surviving turn edits by file, sign, and exact content. Hunk line
//! numbers belong to different baselines and cannot be compared directly.
use std::collections::{HashMap, HashSet};

/// Only unambiguous changed lines are marked. Repeated text is insufficient
/// evidence to attribute a line to a turn, so leave it unmarked.
pub(crate) fn marked_rows(text: &str, turn: &str) -> HashSet<usize> {
    fn changes(text: &str) -> HashMap<(&str, &str), Vec<usize>> {
        let mut changes = HashMap::<_, Vec<_>>::new();
        let mut start = text
            .lines()
            .take_while(|line| !line.starts_with("diff --git "))
            .count();
        for file in crate::files::file_diffs(text) {
            let mut in_hunk = false;
            for (row, line) in file.patch.lines().enumerate() {
                if line.starts_with("@@") {
                    in_hunk = true;
                } else if in_hunk && line.starts_with(['+', '-']) {
                    changes
                        .entry((file.path, line))
                        .or_default()
                        .push(start + row);
                }
            }
            start += file.patch.lines().count();
        }
        changes
    }
    let selected = changes(turn);
    changes(text)
        .into_iter()
        .filter_map(|(key, rows)| {
            (rows.len() == 1 && selected.get(&key).is_some_and(|rows| rows.len() == 1))
                .then(|| rows[0])
        })
        .collect()
}

#[cfg(test)]
mod tests;
