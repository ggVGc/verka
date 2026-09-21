//! A list read from its end, with the operator's place in it.
//!
//! The diagnostic log and the plan-quota readings are the same thing twice:
//! entries in occurrence order, a view anchored to the newest, and a scrollback
//! offset that has to survive entries arriving underneath it. They had eight
//! near-identical methods over four [`App`](crate::app::App) fields; here it is
//! one type used twice.
//!
//! Distinct from [`Scroll`](crate::app::Scroll), which measures a rendered
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
    pub fn restock(&mut self, items: Vec<T>) {
        let grown = items.len().saturating_sub(self.items.len()) as u16;
        self.items = items;
        let max = self.items.len().saturating_sub(1) as u16;
        if self.scroll_back > 0 {
            self.scroll_back = self.scroll_back.saturating_add(grown);
        }
        self.scroll_back = self.scroll_back.min(max);
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
mod tests {
    use super::*;

    fn tail(entries: usize) -> Tail<usize> {
        let mut tail = Tail::default();
        for entry in 0..entries {
            tail.push(entry);
        }
        tail
    }

    /// The point of holding an offset rather than an index: an operator
    /// reading back through the log keeps reading the same entries while the
    /// agent keeps appending to it.
    #[test]
    fn entries_arriving_underneath_do_not_move_a_scrolled_back_view() {
        let mut tail = tail(5);
        tail.scroll_up();
        tail.scroll_up();
        assert_eq!(tail.scroll_back(), 2);

        tail.push(5);
        tail.push(6);

        assert_eq!(tail.scroll_back(), 4, "still on the same two entries back");
    }

    #[test]
    fn a_view_at_the_tail_keeps_following_it() {
        let mut tail = tail(3);
        assert_eq!(tail.scroll_back(), 0);

        tail.push(3);

        assert_eq!(tail.scroll_back(), 0);
    }

    #[test]
    fn scrolling_stops_at_the_first_entry_and_at_the_newest() {
        let mut tail = tail(3);
        for _ in 0..10 {
            tail.scroll_up();
        }
        assert_eq!(tail.scroll_back(), 2, "held at the first entry");

        for _ in 0..10 {
            tail.scroll_down();
        }
        assert_eq!(tail.scroll_back(), 0, "held at the newest");
    }

    #[test]
    fn an_empty_tail_cannot_be_scrolled_anywhere() {
        let mut tail: Tail<usize> = Tail::default();

        tail.scroll_up();
        tail.scroll_to_top();

        assert_eq!(tail.scroll_back(), 0);
        assert!(tail.is_empty());
    }

    /// The timer refill must not fight an operator reading back through the
    /// readings, which a plain `replace` would do every few seconds.
    #[test]
    fn restocking_keeps_the_operators_place() {
        let mut tail = tail(5);
        tail.scroll_up();
        tail.scroll_up();

        tail.restock(vec![0, 1, 2, 3, 4, 5, 6]);

        assert_eq!(tail.scroll_back(), 4, "still on the same two entries back");

        tail.scroll_to_bottom();
        tail.restock(vec![0, 1, 2, 3, 4, 5, 6, 7]);

        assert_eq!(tail.scroll_back(), 0, "a view at the tail follows it");
    }

    /// A shorter reading than the last one — the server trimmed its log —
    /// cannot leave the view pointing past the first entry.
    #[test]
    fn restocking_a_shorter_list_clamps_the_view() {
        let mut tail = tail(5);
        tail.scroll_to_top();

        tail.restock(vec![3, 4]);

        assert_eq!(tail.scroll_back(), 1);
    }

    #[test]
    fn replacing_the_contents_returns_to_the_tail() {
        let mut tail = tail(5);
        tail.scroll_to_top();
        assert_eq!(tail.scroll_back(), 4);

        tail.replace(vec![10, 11]);

        assert_eq!(tail.iter().count(), 2);
        assert_eq!(tail.scroll_back(), 0);
    }
}
