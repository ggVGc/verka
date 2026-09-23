//! Memoization for the line builders that dominate a frame.
//!
//! The event list rebuilds every entry it holds on every frame — see
//! [`crate::event_list::render`] — and a frame is drawn for every keystroke.
//! Syntax highlighting a code block costs roughly two hundred times what
//! rendering it plain does, so a session with a few hundred expanded entries
//! spends tens of milliseconds per frame redoing work whose inputs have not
//! changed since the entry arrived. That is what makes typing lag.
//!
//! The cache is keyed on the whole input — the text and every display choice
//! that shapes the result — so a hit is exact and there is nothing to
//! invalidate. An agent event never changes once it is on the list; a display
//! choice that does change (the link style, the selected citation) is part of
//! the key, so the old rendering simply stops being asked for and ages out.
//!
//! One cache per thread: rendering happens on the thread that owns the
//! terminal, and tests that render in parallel each get their own.

use std::collections::HashMap;
use std::hash::Hash;

/// How many rendered lines to hold before dropping the least recently used
/// half. Bounding by lines rather than by blocks is what keeps one enormous
/// agent message from pinning far more memory than a thousand ordinary ones.
///
/// Twenty thousand lines covers a long session's entire expanded list, which
/// is the case the cache exists for — the whole list is what a frame builds.
const CAPACITY_LINES: usize = 20_000;

/// A cache's own limit on how many renderings it holds, whatever their size,
/// so that a flood of tiny blocks cannot grow the table without bound either.
const CAPACITY_BLOCKS: usize = 4096;

/// How much of the cache a rendering occupies, in lines.
pub(crate) trait Weigh {
    fn weight(&self) -> usize;
}

/// A rendering keyed by everything that produced it.
struct Cached<V> {
    value: V,
    weight: usize,
    /// When this was last asked for, by [`Memo::clock`].
    used: u64,
}

/// A bounded, least-recently-used memo table.
pub(crate) struct Memo<K, V> {
    entries: HashMap<K, Cached<V>>,
    lines: usize,
    clock: u64,
}

impl<K, V> Default for Memo<K, V> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            lines: 0,
            clock: 0,
        }
    }
}

impl<K: Eq + Hash, V: Clone + Weigh> Memo<K, V> {
    /// The cached rendering for `key`, building and storing one if this is the
    /// first time it has been asked for.
    ///
    /// The value is cloned out rather than borrowed: callers go on to indent,
    /// wrap, and mark what they are given, so they need it owned. Cloning
    /// styled lines is an allocation per span — cheap beside the parse,
    /// highlight, and re-parse that building one costs.
    pub(crate) fn get_or_insert_with(&mut self, key: K, build: impl FnOnce() -> V) -> V {
        self.clock += 1;
        let clock = self.clock;
        if let Some(cached) = self.entries.get_mut(&key) {
            cached.used = clock;
            return cached.value.clone();
        }
        let value = build();
        let weight = value.weight();
        // A single rendering larger than the whole cache is handed back
        // uncached: storing it would evict everything else and still not
        // survive the next insertion.
        if weight > CAPACITY_LINES {
            return value;
        }
        if self.lines + weight > CAPACITY_LINES || self.entries.len() >= CAPACITY_BLOCKS {
            self.evict();
        }
        self.lines += weight;
        self.entries.insert(
            key,
            Cached {
                value: value.clone(),
                weight,
                used: clock,
            },
        );
        value
    }

    /// Drop the least recently used half, so eviction is paid for in batches
    /// rather than on every insertion past the limit.
    ///
    /// A list too big for the cache therefore rebuilds the half it evicted on
    /// each frame, which is what an uncached Styra did for every block on
    /// every frame: a cache too small for the session is no worse than no
    /// cache, only no better.
    fn evict(&mut self) {
        let mut used: Vec<u64> = self.entries.values().map(|cached| cached.used).collect();
        if used.is_empty() {
            return;
        }
        used.sort_unstable();
        let cutoff = used[used.len() / 2];
        self.entries.retain(|_, cached| cached.used >= cutoff);
        self.lines = self.entries.values().map(|cached| cached.weight).sum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Weigh for usize {
        fn weight(&self) -> usize {
            1
        }
    }

    #[test]
    fn a_repeated_key_is_built_once() {
        let mut memo: Memo<String, usize> = Memo::default();
        let mut built = 0;
        for _ in 0..3 {
            let value = memo.get_or_insert_with("same".to_owned(), || {
                built += 1;
                7
            });
            assert_eq!(value, 7);
        }
        assert_eq!(built, 1);
    }

    #[test]
    fn a_differing_key_is_built_again() {
        let mut memo: Memo<String, usize> = Memo::default();
        let mut built = 0;
        for key in ["a", "b", "a"] {
            memo.get_or_insert_with(key.to_owned(), || {
                built += 1;
                0
            });
        }
        assert_eq!(built, 2);
    }

    #[test]
    fn the_table_stays_bounded_and_keeps_what_was_asked_for_last() {
        let mut memo: Memo<usize, usize> = Memo::default();
        for key in 0..CAPACITY_BLOCKS + 10 {
            memo.get_or_insert_with(key, || key);
        }
        assert!(
            memo.entries.len() <= CAPACITY_BLOCKS,
            "{}",
            memo.entries.len()
        );
        assert_eq!(
            memo.lines,
            memo.entries.len(),
            "the line count has to track what is actually held"
        );
        let mut built = 0;
        memo.get_or_insert_with(CAPACITY_BLOCKS + 9, || {
            built += 1;
            0
        });
        assert_eq!(built, 0, "the most recent insertion survived the sweep");
    }

    /// A rendering too big to cache is still returned, just not stored.
    #[test]
    fn an_oversized_rendering_is_passed_through() {
        struct Huge;
        impl Weigh for Huge {
            fn weight(&self) -> usize {
                CAPACITY_LINES + 1
            }
        }
        impl Clone for Huge {
            fn clone(&self) -> Self {
                Self
            }
        }
        let mut memo: Memo<&str, Huge> = Memo::default();
        let mut built = 0;
        for _ in 0..2 {
            memo.get_or_insert_with("big", || {
                built += 1;
                Huge
            });
        }
        assert_eq!(built, 2);
        assert!(memo.entries.is_empty());
    }
}
