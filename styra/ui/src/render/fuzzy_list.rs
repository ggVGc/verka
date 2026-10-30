use super::*;
use crate::theme;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};
use ratatui::Frame;

/// Draw a narrowing list into `area`: a bordered box titled `title`, the
/// query along its bottom border once there is one, and the matching rows
/// with their matched letters marked.
pub fn render_fuzzy_list(frame: &mut Frame, view: &FuzzyListView, area: Rect) {
    let border_style = if view.focused {
        Style::default().fg(theme::ACCENT)
    } else {
        Style::default().fg(theme::INACTIVE)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            view.title.to_owned(),
            Style::default().fg(theme::MUTED_TEXT),
        ));
    // The query rides the bottom border rather than taking a row of its own:
    // the box is already as tall as the rows it has, and a list that shrinks
    // by one the moment it is typed at hides the very row being hunted.
    if view.list.is_filtering() {
        block = block.title_bottom(Line::from(vec![
            Span::styled(" /", Style::default().fg(theme::MUTED_TEXT)),
            Span::styled(
                format!("{} ", view.list.query),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
    if !view.hint.is_empty() {
        block = block.title_bottom(
            Line::from(Span::styled(
                view.hint.to_owned(),
                Style::default().fg(theme::MUTED_TEXT),
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
                .fg(theme::MUTED_TEXT)
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
                    theme::SELECTION_MARKER
                } else {
                    theme::TEXT
                }),
            )];
            spans.extend(marked(&view.rows[found.index], &found.positions));
            ListItem::new(Line::from(spans))
        })
        .collect();
    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme::SELECTION_BACKGROUND)
            .add_modifier(Modifier::BOLD),
    );
    let mut state = ListState::default();
    state.select(Some(selected));
    frame.render_stateful_widget(list, area, &mut state);
}

/// `row` cut into spans so the characters at `positions` carry the match
/// mark. The letters that were typed are the reason the row is on screen, so
/// they are what the eye should land on.
pub fn marked(row: &str, positions: &[usize]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut plain = String::new();
    for (index, character) in row.chars().enumerate() {
        if positions.contains(&index) {
            if !plain.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut plain),
                    Style::default().fg(theme::TEXT),
                ));
            }
            spans.push(Span::styled(
                character.to_string(),
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            plain.push(character);
        }
    }
    if !plain.is_empty() {
        spans.push(Span::styled(plain, Style::default().fg(theme::TEXT)));
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

    /// A list keeping its order narrows without ranking: the same query as
    /// above leaves the rows standing in the order they were given.
    #[test]
    fn a_list_keeping_its_order_only_narrows() {
        let rows: Vec<String> = rows().into_iter().rev().collect();
        let mut list = FuzzyList::at(&rows, 0).keeping_order();
        for character in "co5".chars() {
            list.push(character);
        }
        assert_eq!(
            matched_rows(&list, &rows),
            vec!["claude-sonnet-5", "claude-opus-5"]
        );
        assert_eq!(list.selected_row(&rows), Some(1));
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
        list.page_down(&rows);
        list.page_up(&rows);
        assert_eq!(list.selected, 0);
    }

    /// Paging crosses a long list in presses rather than keystrokes, and
    /// stops at its ends rather than wrapping round them.
    #[test]
    fn paging_moves_by_a_page_and_stops_at_the_ends() {
        let rows: Vec<String> = (0..25).map(|index| format!("model-{index:02}")).collect();
        let mut list = FuzzyList::at(&rows, 0);

        list.page_down(&rows);
        assert_eq!(list.selected, PAGE);
        list.page_down(&rows);
        assert_eq!(list.selected, PAGE * 2);
        list.page_down(&rows);
        assert_eq!(list.selected, rows.len() - 1, "the last row, not the first");

        list.page_up(&rows);
        assert_eq!(list.selected, rows.len() - 1 - PAGE);
        for _ in 0..5 {
            list.page_up(&rows);
        }
        assert_eq!(list.selected, 0, "and the first, not the last");
    }

    /// A page is a page of what the query left standing, so paging cannot
    /// leave the cursor past the end of a narrowed list.
    #[test]
    fn paging_is_held_to_the_matching_rows() {
        let rows = rows();
        for (query, count, last_match, last_row) in [("claude-", 3, 2, 2), ("co5", 2, 1, 1)] {
            let mut list = FuzzyList::at(&rows, 0);
            for character in query.chars() {
                list.push(character);
            }
            assert_eq!(list.matches(&rows).len(), count, "{query}");
            list.page_down(&rows);
            assert_eq!(list.selected, last_match, "last match for {query}");
            assert_eq!(list.selected_row(&rows), Some(last_row), "{query}");
            list.page_up(&rows);
            assert_eq!(list.selected, 0, "first match for {query}");
        }
    }

    /// The cursor moves within the matches, not the rows, and holds at both
    /// ends: the top of a ranked list is where the best match is, and stepping
    /// off the bottom must not land there.
    #[test]
    fn the_cursor_moves_within_the_matches_and_holds_at_both_ends() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "claude-".chars() {
            list.push(character);
        }
        assert_eq!(list.matches(&rows).len(), 3);

        list.prev(&rows);
        assert_eq!(list.selected, 0, "already at the top");
        for _ in 0..5 {
            list.next(&rows);
        }
        assert_eq!(
            list.selected, 2,
            "the last match, not back round to the first"
        );
        list.prev(&rows);
        assert_eq!(list.selected, 1);
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

    /// The rows are identifiers, so a word of the query is what the
    /// punctuation in them divides: one press takes back one part of a name,
    /// not the whole thing.
    #[test]
    fn deleting_a_word_takes_back_one_part_of_an_identifier() {
        let rows = rows();
        let mut list = FuzzyList::at(&rows, 0);
        for character in "claude:claude-opus-5/high".chars() {
            list.push(character);
        }

        list.delete_word();
        assert_eq!(list.query, "claude:claude-opus-5/");
        list.delete_word();
        assert_eq!(list.query, "claude:claude-opus-");
        list.delete_word();
        assert_eq!(list.query, "claude:claude-");
        list.delete_word();
        assert_eq!(list.query, "claude:");
        list.delete_word();
        assert!(list.query.is_empty());
        // And on an empty query it is a no-op rather than an underflow.
        list.delete_word();
        assert!(list.query.is_empty());
        assert!(!list.is_filtering());
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
            .filter(|cell| cell.fg == theme::ACCENT && cell.modifier.contains(Modifier::BOLD))
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
