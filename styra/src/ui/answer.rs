//! The last turn's typed answer, rendered as the shape it was asked for.
//!
//! A `files` answer is a navigable list of locations, `lines` a list of items,
//! `json` a pretty-printed document, `text` prose. What they have in common is
//! that the operator asked a question and is owed the answer, not a transcript
//! to find it in — the event list is already there for the transcript.
//!
//! A reply that missed its contract is shown too, and at length: the complaint
//! about its shape is one line, and the message itself is the rest. The agent
//! usually answered; it just did not frame the answer as asked.

use super::{palette, render_placeholder, view_block};
use crate::app::App;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use styra_server::{Answer, AnswerValue, FileLocation};

pub(crate) fn render_answer(frame: &mut Frame, app: &App, area: Rect) {
    let block = view_block(app, Some(&title(app)));

    let Some(answer) = app.answer.answer() else {
        let text = match app.answer.error().as_deref() {
            Some(error) => format!("  {error}"),
            None => "  no answer yet".to_owned(),
        };
        render_placeholder(frame, block, area, &text);
        return;
    };

    let lines = match answer.value.as_ref() {
        Some(value) => value_lines(value, app.answer.selected_index()),
        None => unsatisfied_lines(answer),
    };
    // A navigable answer scrolls to keep the selection in view; a document one
    // is read from the top, and its own keys are not bound to move a selection.
    let viewport = usize::from(area.height.saturating_sub(2));
    let scroll = app
        .answer
        .selected_index()
        .saturating_add(1)
        .saturating_sub(viewport) as u16;
    frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
}

/// What the view is showing, in the block's title: the contract read under,
/// and how many items came back.
fn title(app: &App) -> String {
    let Some(answer) = app.answer.answer() else {
        return "answer".to_owned();
    };
    let contract = answer.contract.as_str();
    match answer.value.as_ref() {
        Some(AnswerValue::Lines(items)) => format!("answer · {contract} · {}", count(items.len())),
        Some(AnswerValue::Files(files)) => format!("answer · {contract} · {}", count(files.len())),
        Some(_) => format!("answer · {contract}"),
        None => format!("answer · {contract} · unsatisfied"),
    }
}

fn count(n: usize) -> String {
    if n == 1 {
        "1 item".to_owned()
    } else {
        format!("{n} items")
    }
}

fn value_lines(value: &AnswerValue, selected: usize) -> Vec<Line<'static>> {
    match value {
        AnswerValue::Text(text) => text
            .lines()
            .map(|line| Line::from(Span::styled(line.to_owned(), Style::default())))
            .collect(),
        AnswerValue::Lines(items) => items
            .iter()
            .enumerate()
            .map(|(index, item)| row(index == selected, vec![Span::raw(item.clone())]))
            .collect(),
        AnswerValue::Files(files) => files
            .iter()
            .enumerate()
            .map(|(index, file)| row(index == selected, file_spans(file)))
            .collect(),
        AnswerValue::Json(json) => serde_json::to_string_pretty(json)
            .unwrap_or_else(|_| json.to_string())
            .lines()
            .map(|line| Line::from(Span::raw(line.to_owned())))
            .collect(),
    }
}

/// The location, then the agent's note about it in a dimmer colour — the path
/// is what the operator is scanning for, the note is why it is in the list.
fn file_spans(file: &FileLocation) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled(
        file.located(),
        Style::default().fg(palette::ACCENT),
    )];
    if !file.description.is_empty() {
        spans.push(Span::styled(
            format!("  {}", file.description),
            Style::default().fg(palette::MUTED_TEXT),
        ));
    }
    spans
}

/// One selectable row, marked and highlighted the way every other list in the
/// interface marks its selection.
fn row(selected: bool, mut spans: Vec<Span<'static>>) -> Line<'static> {
    let marker = if selected { "▍ " } else { "  " };
    let mut line = vec![Span::styled(
        marker,
        Style::default().fg(if selected {
            palette::SELECTION_MARKER
        } else {
            palette::INACTIVE
        }),
    )];
    line.append(&mut spans);
    let style = if selected {
        Style::default().bg(palette::SELECTION_BACKGROUND)
    } else {
        Style::default()
    };
    Line::from(line).style(style)
}

/// A reply that did not satisfy its contract: why, then what was actually
/// said. The message is the useful part, so it gets the room.
fn unsatisfied_lines(answer: &Answer) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            answer
                .error
                .clone()
                .unwrap_or_else(|| "the reply did not satisfy the contract".to_owned()),
            Style::default()
                .fg(palette::WARNING)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "the agent replied:",
            Style::default().fg(palette::ADDITIONAL_INFO),
        )),
        Line::default(),
    ];
    lines.extend(
        answer
            .source
            .lines()
            .map(|line| Line::from(Span::raw(line.to_owned()))),
    );
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::View;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use styra_server::agent::{Provider, Selection};
    use styra_server::Contract;

    fn app_showing(answer: Answer) -> App {
        let mut app = App::new(Selection::new(Provider::Codex), "s-1");
        app.view = View::Answer;
        app.answer.set(Ok(answer));
        app
    }

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
        terminal
            .draw(|frame| super::super::render(frame, app))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(buffer.area().width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn files_answer() -> Answer {
        Answer {
            contract: Contract::Files,
            value: Some(AnswerValue::Files(vec![
                FileLocation {
                    path: PathBuf::from("src/auth.rs"),
                    line: Some(12),
                    column: None,
                    description: "checks the token".into(),
                },
                FileLocation {
                    path: PathBuf::from("src/session.rs"),
                    line: None,
                    column: None,
                    description: String::new(),
                },
            ])),
            error: None,
            source: "…".into(),
        }
    }

    #[test]
    fn a_files_answer_lists_locations_with_their_descriptions() {
        let screen = rendered(&app_showing(files_answer()));
        assert!(screen.contains("src/auth.rs:12"), "{screen}");
        assert!(screen.contains("checks the token"), "{screen}");
        assert!(screen.contains("src/session.rs"), "{screen}");
    }

    /// The title says what was asked for and how much came back, so the
    /// operator can tell an empty answer from an unfetched one.
    #[test]
    fn the_title_names_the_contract_and_the_item_count() {
        let screen = rendered(&app_showing(files_answer()));
        assert!(screen.contains("answer · files · 2 items"), "{screen}");
    }

    #[test]
    fn the_selection_moves_through_a_files_answer() {
        let mut app = app_showing(files_answer());
        assert_eq!(app.answer.rows(), 2);
        app.answer.select_next();
        assert_eq!(
            app.answer.selected_file().map(|file| file.path.clone()),
            Some(PathBuf::from("src/session.rs"))
        );
        // And stops at the end rather than running off it.
        app.answer.select_next();
        assert_eq!(app.answer.selected_index(), 1);
        app.answer.select_prev();
        assert_eq!(app.answer.selected_index(), 0);
    }

    /// The point of keeping `source` on an unsatisfied answer: the operator
    /// sees what the agent said, not only that it was the wrong shape.
    #[test]
    fn an_unsatisfied_answer_shows_the_reply_and_why_it_failed() {
        let screen = rendered(&app_showing(Answer {
            contract: Contract::Json,
            value: None,
            error: Some("the answer block is not valid JSON".into()),
            source: "I could not work out the crate layout.".into(),
        }));
        assert!(screen.contains("not valid JSON"), "{screen}");
        assert!(screen.contains("the agent replied:"), "{screen}");
        assert!(screen.contains("could not work out"), "{screen}");
    }

    #[test]
    fn the_title_marks_an_answer_that_missed_its_contract() {
        let app = app_showing(Answer {
            contract: Contract::Json,
            value: None,
            error: Some("the answer block is not valid JSON".into()),
            source: "…".into(),
        });
        assert_eq!(title(&app), "answer · json · unsatisfied");
    }

    /// A prose or JSON answer has nothing to select, so the navigation keys
    /// have nothing to move and must not pretend otherwise.
    #[test]
    fn a_document_answer_has_no_selectable_rows() {
        let mut app = app_showing(Answer {
            contract: Contract::Text,
            value: Some(AnswerValue::Text("it caches nothing.".into())),
            error: None,
            source: "…".into(),
        });
        assert_eq!(app.answer.rows(), 0);
        app.answer.select_next();
        assert_eq!(app.answer.selected_index(), 0);
        assert!(rendered(&app).contains("it caches nothing."));
    }

    #[test]
    fn a_json_answer_is_pretty_printed() {
        let app = app_showing(Answer {
            contract: Contract::Json,
            value: Some(AnswerValue::Json(
                serde_json::json!({"crate": "styra", "kind": "tui"}),
            )),
            error: None,
            source: "…".into(),
        });
        let screen = rendered(&app);
        assert!(screen.contains("\"crate\": \"styra\""), "{screen}");
    }

    /// Failing to fetch an answer at all is not the same as an answer that
    /// failed to parse, and says so rather than showing an empty view.
    #[test]
    fn a_fetch_failure_is_reported_in_place_of_the_answer() {
        let mut app = App::new(Selection::new(Provider::Codex), "s-1");
        app.view = View::Answer;
        app.answer.set(Err("session has no typed turn to answer".into()));
        assert!(rendered(&app).contains("no typed turn"));
    }
}
