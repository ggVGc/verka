//! A list that narrows as it is typed at.
//!
//! The screens that choose from a fixed catalog — models, agents, tags — all
//! reach a point where the catalog is longer than the box drawn around it, and
//! stepping row by row stops being how anyone finds anything. This is that
//! list: it owns a query, the rows the query leaves standing, and where the
//! cursor sits among them, and it draws the whole thing including the
//! letters each row matched on.
//!
//! Matching is subsequence matching, the same bargain every fuzzy finder
//! makes: `c45` finds `claude-haiku-4-5`. The score then decides the order,
//! and it is built to put the row the operator *meant* first — see
//! [`score`] — unless the list was opened with [`FuzzyList::keeping_order`],
//! in which case the query only narrows and the caller's order stands.

use crate::query::Edit;

/// Rows the cursor moves by one PageUp/PageDown press, matching the lines the
/// rest of the client pages by: the rows here are one line each.
pub const PAGE: usize = 10;

/// A row that survived the query, with the character positions it matched at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    /// Where the row sits in the list it was filtered from, so a caller can
    /// map a choice back onto its own catalog.
    pub index: usize,
    /// Character (not byte) offsets into the row that the query matched.
    pub positions: Vec<usize>,
}

/// The state of a narrowing list: everything about it that a key can change.
///
/// The rows themselves are held by the caller — a model catalog is derived
/// from the selected agent and changes under the list — so this holds only
/// the query and the cursor, and takes the rows on every call.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FuzzyList {
    /// What has been typed. Empty means every row stands.
    pub query: String,
    /// Which of the *matching* rows the cursor is on, not which of `rows`.
    /// Ranking reorders the matches under the cursor, so an index into the
    /// underlying list would not survive a keystroke.
    pub selected: usize,
    /// Whether a query only narrows the rows rather than ranking them. Set for
    /// a list whose own order is the one that matters — most recently used
    /// first — so that order holds while it is typed at.
    pub keeps_order: bool,
}

impl FuzzyList {
    /// A list opened on `row` of `rows`, with nothing typed yet.
    pub fn at(rows: &[String], row: usize) -> Self {
        Self {
            query: String::new(),
            selected: row.min(rows.len().saturating_sub(1)),
            keeps_order: false,
        }
    }

    /// This list, but with the rows a query leaves standing kept in the order
    /// they were given instead of ranked by how well they match.
    pub fn keeping_order(self) -> Self {
        Self {
            keeps_order: true,
            ..self
        }
    }

    /// The rows the query leaves standing, best first — or, for a list
    /// [keeping its order](Self::keeping_order), in the order given. With
    /// nothing typed this is every row in its original order — the caller's
    /// own ordering is a deliberate one (most recently used first, say) and the
    /// list does not get to overrule it until there is a query to rank by.
    pub fn matches(&self, rows: &[String]) -> Vec<Match> {
        if self.query.trim().is_empty() {
            return rows
                .iter()
                .enumerate()
                .map(|(index, _)| Match {
                    index,
                    positions: Vec::new(),
                })
                .collect();
        }
        let mut scored: Vec<(i32, usize, Match)> = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                score(row, &self.query)
                    .map(|(score, positions)| (-score, index, Match { index, positions }))
            })
            .collect();
        // Ties keep the caller's order: `index` breaks them, and the sort is
        // stable on top of that. A list keeping its order is left as
        // collected, which is the order the rows were given in.
        if !self.keeps_order {
            scored.sort_by_key(|(score, index, _)| (*score, *index));
        }
        scored.into_iter().map(|(_, _, found)| found).collect()
    }

    /// Which row of `rows` the cursor names, or `None` when the query has
    /// narrowed the list down to nothing.
    pub fn selected_row(&self, rows: &[String]) -> Option<usize> {
        let matches = self.matches(rows);
        matches
            .get(self.selected.min(matches.len().saturating_sub(1)))
            .map(|found| found.index)
    }

    /// Step the cursor down the matches, stopping at the last one. A no-op
    /// while nothing matches, so an over-narrow query is not also an
    /// arithmetic fault.
    ///
    /// The ends hold rather than wrap. The list is ranked, so its top is where
    /// the best match is: a key held down at the bottom must not carry the
    /// cursor back round to it, and the row under the cursor is the one thing
    /// a press is allowed to change without the eye following it.
    pub fn next(&mut self, rows: &[String]) {
        let count = self.matches(rows).len();
        if count > 0 {
            self.selected = (self.selected + 1).min(count - 1);
        }
    }

    /// Step the cursor up the matches, stopping at the first.
    pub fn prev(&mut self, rows: &[String]) {
        if !self.matches(rows).is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    /// Move the cursor a page down the matches, stopping at the last one, as
    /// [`Self::next`] does.
    pub fn page_down(&mut self, rows: &[String]) {
        let count = self.matches(rows).len();
        if count > 0 {
            self.selected = (self.selected + PAGE).min(count - 1);
        }
    }

    /// Move the cursor a page up the matches, stopping at the first.
    pub fn page_up(&mut self, rows: &[String]) {
        if !self.matches(rows).is_empty() {
            self.selected = self.selected.saturating_sub(PAGE);
        }
    }

    /// Take a typed character into the query. The cursor goes back to the
    /// top: the row it was on is ranked afresh by every keystroke, and
    /// keeping the index would leave it pointing at an unrelated row.
    pub fn push(&mut self, character: char) {
        self.query.push(character);
        self.selected = 0;
    }

    /// Drop the last character of the query.
    pub fn backspace(&mut self) {
        self.query.pop();
        self.selected = 0;
    }

    /// Drop the last word of the query (`Ctrl-W`); see
    /// [`crate::query::delete_word`].
    pub fn delete_word(&mut self) {
        self.edit(Edit::DeleteWord);
    }

    /// Make a change typed into the query, as every search field takes it
    /// (see [`crate::query`]). The cursor goes back to the top, as it does
    /// for [`Self::push`].
    pub fn edit(&mut self, edit: Edit) {
        edit.apply(&mut self.query);
        self.selected = 0;
    }

    /// Whether anything has been typed.
    pub fn is_filtering(&self) -> bool {
        !self.query.is_empty()
    }

    /// Clear the query, leaving the cursor on the row it was on. The row is
    /// passed back in because the cursor counts matches, and dropping the
    /// query restores rows the cursor has to be re-placed among.
    pub fn clear(&mut self, rows: &[String]) {
        let row = self.selected_row(rows);
        self.query.clear();
        self.selected = row.unwrap_or(0);
    }
}

/// How well `query` matches `row`, and where, or `None` if it does not match
/// at all.
///
/// Every query character has to appear in `row`, in order and
/// case-insensitively. Among the rows that clear that bar the score favours,
/// in this order: runs of query characters that are adjacent in the row,
/// matches at the start of a word (a row's own separators — `-`, `_`, `.`,
/// `/`, a digit after a letter), and matches near the front. That is what
/// makes `co5` rank `claude-opus-5` over `claude-sonnet-4-5`, which it also
/// matches.
pub fn score(row: &str, query: &str) -> Option<(i32, Vec<usize>)> {
    let row: Vec<char> = row.chars().collect();
    let mut positions = Vec::new();
    let mut score = 0;
    let mut at = 0usize;
    for wanted in query.chars().filter(|character| !character.is_whitespace()) {
        let found = (at..row.len()).find(|index| {
            row[*index].eq_ignore_ascii_case(&wanted)
                || row[*index].to_lowercase().eq(wanted.to_lowercase())
        })?;
        if positions.last() == Some(&found.wrapping_sub(1)) {
            score += 15;
        }
        if is_word_start(&row, found) {
            score += 10;
        }
        score -= i32::try_from(found - at).unwrap_or(i32::MAX).min(20);
        positions.push(found);
        at = found + 1;
    }
    // A short row that matched used more of itself doing so, which is what
    // separates `claude-opus-5` from a longer row holding the same letters.
    score -= i32::try_from(row.len()).unwrap_or(i32::MAX) / 4;
    Some((score, positions))
}

/// Whether `index` starts a word: the first character, one after a
/// separator, or the first digit of a run following a letter — model names
/// are mostly `word-word-4-5`, and their digits are what gets typed at them.
fn is_word_start(row: &[char], index: usize) -> bool {
    let Some(previous) = index.checked_sub(1).map(|before| row[before]) else {
        return true;
    };
    !previous.is_alphanumeric()
        || (row[index].is_ascii_digit() && previous.is_alphabetic())
        || (row[index].is_uppercase() && previous.is_lowercase())
}

/// What the drawing code is given: the rows, the query over them, and where
/// the cursor is.
pub struct FuzzyListView<'a> {
    pub title: &'a str,
    pub rows: &'a [String],
    pub list: &'a FuzzyList,
    /// Whether this list has the keys. An unfocused one still shows its
    /// query, so a filter left standing on another column is not invisible.
    pub focused: bool,
    /// Shown, dimmed, when no row matches — or when there were none to begin
    /// with. It says which of the two it is.
    pub empty_note: &'a str,
    /// Drawn along the bottom border beside the query: what the keys are, for
    /// a list that is the whole screen it is on. Empty for a list sitting
    /// inside something that says so itself.
    pub hint: &'a str,
}

#[cfg(feature = "renderer")]
pub use rendering::*;

#[cfg(feature = "renderer")]
#[path = "render/fuzzy_list.rs"]
mod rendering;
