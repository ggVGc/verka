use super::super::test_support;
use crate::app::App;

use styra_protocol::event::AgentEvent;

#[test]
fn highlighted_answers_preview_the_selected_section_even_when_following_commands() {
    let mut app = test_support::app("s1");
    app.push_event(AgentEvent::UserMessage {
        text: styra_protocol::contract::frame("show me", styra_protocol::Contract::Lines),
    });
    app.push_event(AgentEvent::CommandStarted {
        command: "echo waiting".into(),
    });
    app.push_event(AgentEvent::AgentMessage {
            text: "Before\n<styra:answer>\nfirst answer\n</styra:answer>\nBetween\n<styra:answer>\nsecond answer\n</styra:answer>\n[web](https://example.com)".into(),
        });
    app.preview.show();
    app.preview.toggle_target();
    app.highlight_first_link();

    let first = super::view(&app, false)
        .answer_target
        .expect("first answer");
    assert_eq!(first, app.highlighted_answer().unwrap());
    app.highlight_next_link();
    let second = super::view(&app, false)
        .answer_target
        .expect("second answer");
    assert_eq!(second, app.highlighted_answer().unwrap());
    assert_ne!(first, second);

    app.highlight_next_link();
    assert!(super::view(&app, false).answer_target.is_none());
    app.highlight_prev_link();
    assert!(super::view(&app, false).answer_target.is_some());
    app.clear_link_highlight();
    assert!(super::view(&app, false).answer_target.is_none());
    assert!(app.preview.open);
    assert!(app.preview.follows_command());
}

#[test]
fn file_link_data_contains_content_position_and_scroll_requests() {
    use super::super::apply_feedback;
    let root = std::env::temp_dir().join("styra-preview-file-scroll");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let file = (1..=200)
        .map(|n| format!("row {n:03}\n"))
        .collect::<String>();
    std::fs::write(root.join("long.txt"), file).unwrap();
    let mut app = test_support::app("s1");
    app.workspace.enter(root.clone());
    app.push_event(AgentEvent::AgentMessage {
        text: "[long](long.txt:100)".into(),
    });
    app.preview.show();
    app.highlight_first_link();
    let view = super::view(&app, false);
    let target = view.file_target.unwrap();
    assert_eq!(target.line, Some(100));
    assert!(
        matches!(target.content, styra_ui::preview::FileTargetContent::Ready(ref content) if content.lines().count() == 200)
    );
    let feedback = styra_ui::RenderFeedback {
        scroll: vec![styra_ui::ScrollFeedback {
            panel: styra_ui::PanelId::Preview {
                session: "s1".into(),
                target: styra_ui::PreviewPanel::Selection,
            },
            limit: 175,
            effective_offset: 99,
            viewport: 25,
        }],
        ..Default::default()
    };
    apply_feedback(&mut app, &feedback);
    assert_eq!(super::view(&app, false).requested_scroll, 99);
    app.preview.scroll.half_page_down();
    assert!(super::view(&app, false).requested_scroll > 99);
    app.preview.scroll.scroll_to_end();
    assert_eq!(super::view(&app, false).requested_scroll, u16::MAX);
    let feedback = styra_ui::RenderFeedback {
        scroll: vec![styra_ui::ScrollFeedback {
            panel: styra_ui::PanelId::Preview {
                session: "s1".into(),
                target: styra_ui::PreviewPanel::Selection,
            },
            limit: 175,
            effective_offset: 175,
            viewport: 25,
        }],
        ..Default::default()
    };
    apply_feedback(&mut app, &feedback);
    assert_eq!(super::view(&app, false).requested_scroll, 175);

    app.clear_link_highlight();
    assert_eq!(app.preview.scroll.offset, 0);
    let _ = std::fs::remove_dir_all(&root);
}

fn changed(path: &str, old: &str, new: &str) -> AgentEvent {
    AgentEvent::FileChanged {
        id: String::new(),
        paths: vec![path.into()],
        diff: Some(format!("@@ edit @@\n-{old}\n+{new}")),
        checkpoint: None,
        checkpoint_error: None,
    }
}

/// Two turns, each a request and the work done for it, with the preview
/// open over the conversation-only list.
fn app_with_two_turns() -> App {
    let mut app = test_support::app("s1");
    app.timeline.all_events = false;
    app.push_event(AgentEvent::UserMessage {
        text: "fix the retry backoff".into(),
    });
    app.push_event(AgentEvent::CommandStarted {
        command: "cargo test backoff".into(),
    });
    app.push_event(changed("src/retry.rs", "delay * 3", "delay * 2"));
    app.push_event(AgentEvent::UserMessage {
        text: "and rename the helper".into(),
    });
    app.push_event(changed("src/helper.rs", "fn old_name", "fn new_name"));
    app.select_first();
    app.preview.show();
    app
}

/// A message previews as itself: its turn's diff is the diff panel's to
/// show (see [`crate::turn_diff`]).
#[test]
fn a_selected_message_previews_as_itself() {
    let app = app_with_two_turns();
    let view = super::view(&app, false);
    assert!(
        matches!(view.entry.unwrap().event, AgentEvent::UserMessage { text } if text == "fix the retry backoff")
    );
    assert!(view.entry_change.is_none());
}

/// A file change is its own diff, its path from the Workspace on, and a
/// Claude edit numbered from where it sits in the file.
#[test]
fn a_file_change_previews_its_own_diff() {
    let dir = std::env::temp_dir().join(format!("styra-snippet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("retry.rs");
    std::fs::write(&file, "fn a() {}\nfn b() {}\nlet delay = base * 2;\n").unwrap();
    let workspace = styra_protocol::agent::SandboxLayout::default().workspace;

    let mut app = test_support::app("s1");
    app.push_event(AgentEvent::UserMessage {
        text: "fix the retry backoff".into(),
    });
    app.push_event(AgentEvent::FileChanged {
        id: String::new(),
        paths: vec![file.display().to_string()],
        diff: Some("@@ edit @@\n-let delay = base * 3;\n+let delay = base * 2;".into()),
        checkpoint: None,
        checkpoint_error: None,
    });
    app.push_event(changed(
        &workspace.join("src/helper.rs").display().to_string(),
        "fn old_name",
        "fn new_name",
    ));
    app.select_first();
    app.select_next_line();
    app.preview.show();
    let view = super::view(&app, false);
    let change = view.entry_change.unwrap();
    assert_eq!(change.paths, [file.display().to_string()]);
    assert!(change.diff.unwrap().contains("@@ -3,1 +3,1 @@"));
    app.select_next_line();
    let view = super::view(&app, false);
    assert_eq!(
        view.entry_change.unwrap().paths,
        [workspace.join("src/helper.rs").display().to_string()]
    );
    assert!(view
        .workspace_roots
        .contains(&workspace.display().to_string()));
    let _ = std::fs::remove_dir_all(dir);
}
