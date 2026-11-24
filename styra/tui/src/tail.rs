//! A list read from its end, with the operator's place in it.
//!
//! The diagnostic log and the plan-quota readings are the same thing twice:
//! entries in occurrence order, a view anchored to the newest, and a scrollback
//! offset that has to survive entries arriving underneath it. They had eight
//! near-identical methods over four [`App`](crate::app::App) fields; here it is
//! one type used twice.
//!
//! Distinct from [`Scroll`](styra_ui::scroll::Scroll), which measures a rendered
//! panel in lines and is clamped by whatever the renderer could fit. This
//! counts entries, which are known without rendering.

/// Entries in occurrence order, and how far back from the newest the view is.
pub struct Tail<T> {
    items: Vec<T>,
    /// Entries scrolled back from the newest; 0 tracks the tail.
    scroll_back: u16,
}

impl<T> Default for Tail<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            scroll_back: 0,
        }
    }
}

impl<T> Tail<T> {
    /// Append an entry. A view already scrolled back stays on the entries it
    /// was showing rather than being pulled along by the new tail; a view at
    /// the tail keeps following it.
    pub fn push(&mut self, item: T) {
        self.items.push(item);
        if self.scroll_back > 0 {
            self.scroll_back = self.scroll_back.saturating_add(1);
        }
    }

    /// Replace the contents wholesale and return to the tail, for a view
    /// filled by asking the server rather than by accumulating.
    pub fn replace(&mut self, items: Vec<T>) {
        self.items = items;
        self.scroll_back = 0;
    }

    /// Take a fresh reading of the same list without moving the operator: a
    /// view at the tail follows the new entries, a view scrolled back stays on
    /// the entries it was reading, exactly as [`push`](Self::push) does.
    ///
    /// For a list refilled on a timer rather than on request, where
    /// [`replace`](Self::replace)'s jump to the tail would fight the operator.
    ///
    /// Reports whether the reading differed from the one it replaced. A timer
    /// that refills a list with what it already held has changed nothing, and
    /// the loop that called it needs to be able to tell that from a refill
    /// that brought something new.
    pub fn restock(&mut self, items: Vec<T>) -> bool
    where
        T: PartialEq,
    {
        let changed = self.items != items;
        let grown = items.len().saturating_sub(self.items.len()) as u16;
        self.items = items;
        let max = self.items.len().saturating_sub(1) as u16;
        if self.scroll_back > 0 {
            self.scroll_back = self.scroll_back.saturating_add(grown);
        }
        self.scroll_back = self.scroll_back.min(max);
        changed
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter()
    }

    /// The newest entry, which is the one a view anchored to the tail shows
    /// last.
    #[cfg(test)]
    pub fn newest(&self) -> Option<&T> {
        self.items.last()
    }

    /// How far back from the newest entry the view is.
    pub fn scroll_back(&self) -> u16 {
        self.scroll_back
    }

    /// Scroll one entry towards the start, stopping at the first one.
    pub fn scroll_up(&mut self) {
        let max = self.items.len().saturating_sub(1) as u16;
        self.scroll_back = self.scroll_back.saturating_add(1).min(max);
    }

    /// Scroll one entry towards the newest.
    pub fn scroll_down(&mut self) {
        self.scroll_back = self.scroll_back.saturating_sub(1);
    }

    /// Jump to the oldest entry.
    pub fn scroll_to_top(&mut self) {
        self.scroll_back = self.items.len().saturating_sub(1) as u16;
    }

    /// Jump back to the newest, resuming the tail.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_back = 0;
    }
}

#[cfg(test)]
mod tests;
