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
//! [`score`].

use crate::palette;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

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
}

impl FuzzyList {
    /// A list opened on `row` of `rows`, with nothing typed yet.
    pub fn at(rows: &[String], row: usize) -> Self {
        Self {
            query: String::new(),
            selected: row.min(rows.len().saturating_sub(1)),
        }
    }

    /// The rows the query leaves standing, best first. With nothing typed
    /// this is every row in its original order — the caller's own ordering is
    /// a deliberate one (most recently used first, say) and the list does not
    /// get to overrule it until there is a query to rank by.
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
        // stable on top of that.
        scored.sort_by_key(|(score, index, _)| (*score, *index));
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

    /// Step the cursor down the matches, wrapping. A no-op while nothing
    /// matches, so an over-narrow query is not also an arithmetic fault.
    pub fn next(&mut self, rows: &[String]) {
        let count = self.matches(rows).len();
        if count > 0 {
            self.selected = (self.selected + 1) % count;
        }
    }

    /// Step the cursor up the matches, wrapping.
    pub fn prev(&mut self, rows: &[String]) {
        let count = self.matches(rows).len();
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
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

/// Draw a narrowing list into `area`: a bordered box titled `title`, the
/// query along its bottom border once there is one, and the matching rows
/// with their matched letters marked.
pub fn render_fuzzy_list(frame: &mut Frame, view: &FuzzyListView, area: Rect) {
    let border_style = if view.focused {
        Style::default().fg(palette::ACCENT)
    } else {
        Style::default().fg(palette::INACTIVE)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            view.title.to_owned(),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    // The query rides the bottom border rather than taking a row of its own:
    // the box is already as tall as the rows it has, and a list that shrinks
    // by one the moment it is typed at hides the very row being hunted.
    if view.list.is_filtering() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(" /", Style::default().fg(palette::MUTED_TEXT)),
            Span::styled(
                format!("{} ", view.list.query),
                Style::default()
                    .fg(palette::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !view.hint.is_empty() {
        block = block.title_bottom(
            Line::from(Span::styled(
                view.hint.to_owned(),
                Style::default().fg(palette::MUTED_TEXT),
            ))
            .right_aligned(),
        );
    }

    let matches = view.list.matches(view.rows);
    if matches.is_empty() {
        let note = if view.rows.is_empty() {
            view.empty_note.to_owned()
        } else {
            format!("no match for {}", view.list.query)
        };
        let note = ListItem::new(Line::from(Span::styled(
            format!("  {note}"),
            Style::default()
                .fg(palette::MUTED_TEXT)
                .add_modifier(Modifier::DIM),
        )));
        frame.render_widget(List::new(vec![note]).block(block), area);
        return;
    }

    let selected = view.list.selected.min(matches.len() - 1);
    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(row, found)| {
            let mut spans = vec![Span::styled(
                if row == selected { "• " } else { "  " },
                Style::default().fg(if row == selected {
                    palette::SELECTION_MARKER
                } else {
                    palette::TEXT
                }),
            )];
            spans.extend(marked(&view.rows[found.index], &found.positions));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(palette::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// `row` cut into spans so the characters at `positions` carry the match
/// mark. The letters that were typed are the reason the row is on screen, so
/// they are what the eye should land on.
fn marked(row: &str, positions: &[usize]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    for (index, character) in row.chars().enumerate() {
        if positions.contains(&index) {
            if !plain.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut plain),
                    Style::default().fg(palette::TEXT),
                ));
            }
            spans.push(Span::styled(
                character.to_string(),
                Style::default()
                    .fg(palette::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            plain.push(character);
        }
    }
    if !plain.is_empty() {
        spans.push(Span::styled(plain, Style::default().fg(palette::TEXT)));
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn rows() -> Vec<String> {
        [
            "claude-opus-5",
            "claude-sonnet-5",
            "claude-haiku-4-5-20251001",
        ]
        .iter()
        .map(|row| (*row).to_owned())
        .collect()
    }

    fn matched_rows(list: &FuzzyList, rows: &[String]) -> Vec<String> {
        list.matches(rows)
            .into_iter()
            .map(|found| rows[found.index].clone())
            .collect()
    }

    #[test]
    fn an_empty_query_keeps_every_row_in_the_order_it_was_given() {
        let rows = rows();
        let list = FuzzyList::at(&rows, 1);
        assert_eq!(matched_rows(&list, &rows), rows);
        assert_eq!(list.selected_row(&rows), Some(1));
    }

    /// The point of the thing: initials and digits scattered through a name
    /// find it, without the separators between them being typed.
    #[test]
    fn scattered_letters_find_the_row_they_were_taken_from() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "chk45".chars() {
            list.push(character);
        }
        assert_eq!(
            matched_rows(&list, &rows),
            vec!["claude-haiku-4-5-20251001"]
        );
    }

    /// Several rows can match the same letters; the one whose words they
    /// start is the one meant.
    #[test]
    fn word_starts_outrank_letters_found_mid_word() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "co5".chars() {
            list.push(character);
        }
        let matched = matched_rows(&list, &rows);
        assert_eq!(matched.first().map(String::as_str), Some("claude-opus-5"));
        assert!(
            matched.contains(&"claude-sonnet-5".to_owned()),
            "the other row still matches, it just ranks lower: {matched:?}"
        );
    }

    #[test]
    fn a_query_matching_nothing_leaves_no_rows_and_no_selection() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "zzz".chars() {
            list.push(character);
        }
        assert!(list.matches(&rows).is_empty());
        assert_eq!(list.selected_row(&rows), None);
        // And moving within nothing is a no-op rather than a panic.
        list.next(&rows);
        list.prev(&rows);
        assert_eq!(list.selected, 0);
    }

    #[test]
    fn the_cursor_wraps_over_the_matches_rather_than_the_rows() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "claude-".chars() {
            list.push(character);
        }
        assert_eq!(list.matches(&rows).len(), 3);
        list.prev(&rows);
        assert_eq!(list.selected, 2);
        list.next(&rows);
        assert_eq!(list.selected, 0);
    }

    /// Clearing the query puts the operator back in the full list on the row
    /// they had narrowed down to, not back at the top of it.
    #[test]
    fn clearing_the_query_keeps_the_row_the_cursor_had_reached() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "haiku".chars() {
            list.push(character);
        }
        assert_eq!(list.selected_row(&rows), Some(2));
        list.clear(&rows);
        assert!(!list.is_filtering());
        assert_eq!(list.selected_row(&rows), Some(2));
    }

    #[test]
    fn backspacing_widens_the_list_again() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "haiku".chars() {
            list.push(character);
        }
        assert_eq!(list.matches(&rows).len(), 1);
        for _ in 0..5 {
            list.backspace();
        }
        assert_eq!(list.matches(&rows).len(), 3);
        assert!(!list.is_filtering());
    }

    fn rendered(view: &FuzzyListView) -> String {
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        terminal
            .draw(|frame| render_fuzzy_list(frame, view, frame.area()))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn the_query_is_drawn_with_the_rows_it_left_standing() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "haiku".chars() {
            list.push(character);
        }
        let screen = rendered(&FuzzyListView {
            title: " model ",
            rows: &rows,
            list: &list,
            focused: true,
            empty_note: "",
            hint: "",
        });
        assert!(screen.contains("model"), "{screen}");
        assert!(
            screen.contains("/haiku"),
            "the query is on screen: {screen}"
        );
        assert!(screen.contains("claude-haiku"), "{screen}");
        assert!(!screen.contains("opus"), "the filtered-out rows are gone");
    }

    #[test]
    fn the_matched_letters_are_marked() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "opus".chars() {
            list.push(character);
        }
        let mut terminal = Terminal::new(TestBackend::new(40, 8)).unwrap();
        let view = FuzzyListView {
            title: " model ",
            rows: &rows,
            list: &list,
            focused: true,
            empty_note: "",
            hint: "",
        };
        terminal
            .draw(|frame| render_fuzzy_list(frame, &view, frame.area()))
            .unwrap();
        let marked: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .filter(|cell| cell.fg == palette::ACCENT && cell.modifier.contains(Modifier::BOLD))
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            marked.contains("opus"),
            "matched letters stand out: {marked}"
        );
    }

    /// A query that matches nothing says so, rather than leaving a blank box
    /// that reads as a list with nothing in it.
    #[test]
    fn an_over_narrow_query_says_nothing_matched() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "zzz".chars() {
            list.push(character);
        }
        let screen = rendered(&FuzzyListView {
            title: " model ",
            rows: &rows,
            list: &list,
            focused: true,
            empty_note: "nothing here",
            hint: "",
        });
        assert!(screen.contains("no match for zzz"), "{screen}");
    }

    /// An empty list and an over-narrowed one are different states: the first
    /// gets the caller's own note.
    #[test]
    fn a_list_with_no_rows_shows_the_note_it_was_given() {
        let screen = rendered(&FuzzyListView {
            title: " effort ",
            rows: &[],
            list: &FuzzyList::default(),
            focused: false,
            empty_note: "none for this model",
            hint: "",
        });
        assert!(screen.contains("none for this model"), "{screen}");
    }
}
