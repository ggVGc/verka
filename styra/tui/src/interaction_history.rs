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
mod tests;
