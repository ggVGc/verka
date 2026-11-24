use super::*;
use ratatui::{backend::TestBackend, Terminal};
use styra_ui::chrome::{PanelChrome, StatusTone};

#[test]
fn navigation_classifies_removed_rows_without_inspecting_rendered_styles() {
    for patch in [
        "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
        "@@ -1,2 +1,2 @@\n---literal minus\n+new\n context\n\\ No newline at end of file\n",
        "-old\n+new\n",
    ] {
        let rendered = styra_ui::diff::diff_block_lines(patch, None, false);
        let expected: Vec<_> = rendered.iter().map(is_removed).collect();
        assert_eq!(removed_rows(patch), expected, "{patch}");
    }
}

#[test]
fn semantic_chrome_intro_and_conversation_are_styled_by_the_renderer() {
    use styra_protocol::event::{AgentEvent, Protocol};
    use styra_ui::markdown::LinkDisplay;
    let prompt = AgentEvent::UserMessage {
        text: "fix the retry backoff".into(),
    };
    let reply = AgentEvent::AgentMessage {
        text: "the retries now work".into(),
    };
    let chrome = PanelChrome {
        focused: true,
        workspace: Some("payments".into()),
        session: None,
        worktree: None,
        source_branch: None,
        into: None,
        parent: None,
        agent: "codex".into(),
        model: "model".into(),
        model_reported: true,
        effort: None,
        effort_reported: true,
        status: "running".into(),
        status_tone: StatusTone::Running,
        elapsed: None,
        suffix: Some("turn diff".into()),
    };
    let mut view = CheckoutDiffView::from_chrome(chrome);
    view.diff = Ok("diff --git a/a b/a\n@@ -1 +1 @@\n-old\n+new\n");
    view.intro_message = Some(styra_ui::preview::MessageView {
        event: &prompt,
        protocol: Protocol::default(),
        links: LinkDisplay::Compact,
    });
    view.conversation_data = Some(styra_ui::picker::conversation(
        [&prompt, &reply].into_iter(),
        Protocol::default(),
        2,
    ));
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal
        .draw(|frame| {
            render(frame, &view, frame.area());
        })
        .unwrap();
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(120)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    // The agent and its state are the footer's and the log's to show.
    assert!(rows[0].starts_with("┏ payments · turn diff"), "{}", rows[0]);
    assert!(!rows[0].contains("codex"));
    let text = rows.concat();
    assert!(text.contains("fix the retry backoff"));
    assert!(text.contains("the retries now work"));
    assert!(text.contains("new"));
}
