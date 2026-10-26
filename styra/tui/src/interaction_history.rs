//! Client-wide history of explicitly entered interactions, across Workspaces.

const MAX_ENTRIES: usize = 100;

#[derive(Default)]
pub struct InteractionHistory {
    entries: Vec<String>,
    cursor: Option<usize>,
}

impl InteractionHistory {
    pub fn record(&mut self, id: &str) {
        if id.is_empty() || self.cursor.is_some_and(|index| self.entries[index] == id) {
            return;
        }
        self.entries
            .truncate(self.cursor.map_or(0, |index| index + 1));
        self.entries.push(id.to_owned());
        if self.entries.len() > MAX_ENTRIES {
            self.entries.remove(0);
        }
        self.cursor = Some(self.entries.len() - 1);
    }

    /// Previewing a row does not move the cursor. Back first returns to the
    /// committed entry; a failed load likewise leaves the cursor untouched.
    pub fn target(&self, current: &str, forward: bool) -> Option<(usize, &str)> {
        let cursor = self.cursor?;
        let index = if forward {
            cursor.checked_add(1)?
        } else if self.entries[cursor] != current {
            cursor
        } else {
            cursor.checked_sub(1)?
        };
        Some((index, self.entries.get(index)?.as_str()))
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn arrive(&mut self, index: usize) {
        self.cursor = Some(index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_and_forward_preserve_entries_until_a_new_choice() {
        let mut history = InteractionHistory::default();
        for id in ["a", "b", "c"] {
            history.record(id);
        }
        assert_eq!(history.target("c", false), Some((1, "b")));
        history.arrive(1);
        history.record("b");
        assert_eq!(history.target("b", true), Some((2, "c")));
        history.record("d");
        assert_eq!(history.entries, ["a", "b", "d"]);
        assert_eq!(history.target("d", true), None);
    }

    #[test]
    fn previews_and_failed_loads_leave_the_committed_position_intact() {
        let mut history = InteractionHistory::default();
        history.record("a");
        history.record("b");
        assert_eq!(history.target("preview", false), Some((1, "b")));
        assert_eq!(history.target("b", false), Some((0, "a")));
        assert_eq!(history.target("b", false), Some((0, "a")));
    }

    #[test]
    fn history_is_bounded_and_ignores_blank_and_repeated_entries() {
        let mut history = InteractionHistory::default();
        history.record("");
        assert_eq!(history.target("", false), None);
        for index in 0..MAX_ENTRIES + 10 {
            history.record(&index.to_string());
            history.record(&index.to_string());
        }
        assert_eq!(history.entries.len(), MAX_ENTRIES);
        assert_eq!(history.entries[0], "10");
        for index in (0..MAX_ENTRIES - 1).rev() {
            let current = history.entries[history.cursor.unwrap()].clone();
            assert_eq!(history.target(&current, false).unwrap().0, index);
            history.arrive(index);
        }
        assert_eq!(history.target("10", false), None);
    }
}
