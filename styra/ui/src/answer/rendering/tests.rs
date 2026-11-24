use super::*;
use crate::chrome::StatusTone;
use ratatui::{backend::TestBackend, Terminal};
use std::path::PathBuf;
use styra_protocol::Contract;

fn chrome() -> PanelChrome {
    PanelChrome {
        focused: true,
        workspace: Some("work".into()),
        worktree: None,
        into: None,
        source_branch: None,
        parent: None,
        agent: "codex".into(),
        model: "gpt".into(),
        model_reported: true,
        effort: None,
        effort_reported: false,
        status: "idle".into(),
        status_tone: StatusTone::Idle,
        elapsed: None,
        suffix: None,
        session: None,
    }
}

fn screen(answer: Option<&Answer>, error: Option<&str>, selected: usize) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| {
            render(
                frame,
                &AnswerView {
                    chrome: chrome(),
                    answer,
                    error,
                    selected,
                },
                frame.area(),
            )
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .chunks(80)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn answer_sections_render_as_links_and_leave_surrounding_prose() {
    use styra_protocol::event::{AgentEvent, DetailBlock, Protocol};
    let text = "Before\n<styra:answer>\nsecret.rs\n</styra:answer>\nAfter";
    let event = AgentEvent::AgentMessage { text: text.into() };
    let blocks = presented_detail(&event, Protocol::CodexJsonl);
    let DetailBlock::Text(output) = &blocks[0] else {
        panic!("expected text")
    };
    assert_eq!(output, "Before\n[answer](styra:answer:0)\nAfter");
    assert_eq!(crate::markdown::markdown_link_count(output), 1);
    assert_eq!(
        crate::markdown::markdown_link_destination(output, 0).as_deref(),
        Some("styra:answer:0")
    );
    let lines = crate::event_list::message_rows(
        &event,
        None,
        Default::default(),
        80,
        Protocol::CodexJsonl,
        crate::markdown::LinkDisplay::Compact,
    );
    let screen = lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!screen.contains("secret.rs"), "{screen}");
    assert!(screen.contains("answer"), "{screen}");
    assert!(
        screen.contains("Before") && screen.contains("After"),
        "{screen}"
    );
}

#[test]
fn files_show_count_locations_notes_and_selection() {
    let answer = Answer {
        contract: Contract::Files,
        value: Some(AnswerValue::Files(vec![
            FileLocation {
                path: PathBuf::from("src/auth.rs"),
                line: Some(12),
                column: None,
                end_line: None,
                description: "checks token".into(),
            },
            FileLocation {
                path: PathBuf::from("src/session.rs"),
                line: None,
                column: None,
                end_line: None,
                description: String::new(),
            },
        ])),
        error: None,
        source: String::new(),
    };
    let output = screen(Some(&answer), None, 1);
    assert!(output.contains("answer · files · 2 items"), "{output}");
    assert!(output.contains("src/auth.rs:12  checks token"), "{output}");
    assert!(output.contains("▍ src/session.rs"), "{output}");
}

#[test]
fn json_is_pretty_printed() {
    let answer = Answer {
        contract: Contract::Json,
        value: Some(AnswerValue::Json(serde_json::json!({"crate": "styra"}))),
        error: None,
        source: String::new(),
    };
    assert!(screen(Some(&answer), None, 0).contains("\"crate\": \"styra\""));
}

#[test]
fn unsatisfied_answer_keeps_error_and_source() {
    let answer = Answer {
        contract: Contract::Json,
        value: None,
        error: Some("invalid JSON".into()),
        source: "plain reply".into(),
    };
    let output = screen(Some(&answer), None, 0);
    assert!(output.contains("unsatisfied"));
    assert!(output.contains("invalid JSON"));
    assert!(output.contains("plain reply"));
}

#[test]
fn missing_and_failed_fetches_are_distinct() {
    assert!(screen(None, None, 0).contains("no answer yet"));
    assert!(screen(None, Some("fetch failed"), 0).contains("fetch failed"));
}
