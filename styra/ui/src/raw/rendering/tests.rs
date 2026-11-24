use super::*;
use crate::chrome::StatusTone;
use ratatui::{backend::TestBackend, Terminal};

fn chrome() -> PanelChrome {
    PanelChrome {
        focused: true,
        workspace: None,
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
fn screen(view: RawView<'_>) -> (String, RawFeedback) {
    let mut terminal = Terminal::new(TestBackend::new(90, 14)).unwrap();
    let mut feedback = RawFeedback::default();
    terminal
        .draw(|frame| feedback = render(frame, &view, frame.area()))
        .unwrap();
    let output = terminal
        .backend()
        .buffer()
        .content()
        .chunks(90)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n");
    (output, feedback)
}
#[test]
fn raw_sources_show_their_titles_markers_and_preview() {
    let wire = vec![
        RawLine {
            at_ms: 0,
            direction: WireDirection::ToAgent,
            text: r#"{"method":"item/start","params":{"n":1}}"#.into(),
        },
        RawLine {
            at_ms: 0,
            direction: WireDirection::FromAgent,
            text: r#"{"type":"system","subtype":"init","ok":true}"#.into(),
        },
    ];
    let provider = vec![r#"{"type":"session_meta","payload":{"id":"t-1"}}"#.to_owned()];
    for (name, source, selected, present, absent) in [
        (
            "received",
            RawSource::Received(&[]),
            0,
            vec!["messages", "no received messages yet"],
            vec!["v: provider native"],
        ),
        (
            "wire",
            RawSource::Styra(&wire),
            1,
            vec!["»", "«", "item/start", "system:init", "\"ok\"", "true"],
            vec![],
        ),
        (
            "provider",
            RawSource::Provider(&provider),
            0,
            vec!["provider native", "session_meta"],
            vec!["« session_meta"],
        ),
    ] {
        let (output, _) = screen(RawView {
            chrome: chrome(),
            source,
            selected,
            requested_preview_scroll: 0,
        });
        for needle in present {
            assert!(
                output.contains(needle),
                "{name}: missing {needle}: {output}"
            );
        }
        for needle in absent {
            assert!(
                !output.contains(needle),
                "{name}: unexpected {needle}: {output}"
            );
        }
    }
}

#[test]
fn untagged_text_is_not_hidden() {
    let line = RawLine {
        at_ms: 0,
        direction: WireDirection::FromAgent,
        text: "not json".into(),
    };
    assert_eq!(wire_line(&line, false).spans[1].content, "not json");
}
#[test]
fn preview_feedback_clamps_long_documents() {
    let value = serde_json::json!({"items": (0..40).collect::<Vec<_>>()});
    let lines = vec![RawLine {
        at_ms: 0,
        direction: WireDirection::FromAgent,
        text: value.to_string(),
    }];
    let (_, feedback) = screen(RawView {
        chrome: chrome(),
        source: RawSource::Styra(&lines),
        selected: 0,
        requested_preview_scroll: u16::MAX,
    });
    assert!(feedback.preview_limit > 0);
    assert_eq!(feedback.effective_preview_scroll, feedback.preview_limit);
}
