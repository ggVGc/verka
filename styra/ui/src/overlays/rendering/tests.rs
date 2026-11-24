use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn rendered(prompt: InsertPromptView<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
    terminal
        .draw(|frame| render_insert(frame, Some(prompt), frame.area()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn rendered_tags(available: &[String], selected: &[String]) -> String {
    let list = FuzzyList::at(available, 0);
    let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_tags(
                frame,
                TagPickerView {
                    available,
                    selected,
                    list: &list,
                    new_tag: None,
                    filter: true,
                },
            )
        })
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
fn action_menu_names_its_window_and_narrows_to_the_query() {
    let rows = vec!["quit (q)".to_owned(), "log view (ctrl-l)".to_owned()];
    let mut list = FuzzyList::at(&rows, 0);
    for character in "log".chars() {
        list.push(character);
    }
    let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_action_menu(
                frame,
                ActionMenuView {
                    window: "events",
                    rows: &rows,
                    list: &list,
                },
                frame.area(),
            )
        })
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("events actions"), "{screen}");
    assert!(screen.contains("log view (ctrl-l)"), "{screen}");
    assert!(!screen.contains("quit (q)"), "{screen}");
    assert!(screen.contains("/log"), "the query is shown: {screen}");
}

#[test]
fn rebase_status_box_shows_progress_and_final_outcomes_but_not_model_output() {
    for (finished, succeeded, title) in [
        (false, false, "rebasing interaction"),
        (true, true, "rebase completed"),
        (true, false, "rebase stopped"),
    ] {
        let mut terminal = Terminal::new(TestBackend::new(90, 5)).unwrap();
        let status = styra_protocol::RebaseStatus {
            finished,
            succeeded,
            message: "Resolving conflicts in file.rs".into(),
            output: "MODEL: Resolved file.rs and continuing the rebase".into(),
            source_commit: "abc123".into(),
        };
        assert_eq!(StatusView::rebase(&status).height(90), 3);
        terminal
            .draw(|frame| StatusView::rebase(&status).render(frame, frame.area()))
            .unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains(title));
        assert!(screen.contains(&status.message));
        assert!(!screen.contains(&status.output));
        assert_eq!(screen.contains("Enter / Esc close"), finished);
        assert_eq!(screen.contains("locked while rebasing"), !finished);
    }
}

#[test]
fn rebase_status_box_grows_with_a_long_message_up_to_a_limit() {
    let mut status = styra_protocol::RebaseStatus {
        finished: true,
        succeeded: false,
        message: "Rebase stopped: a reason that wraps ".repeat(2),
        source_commit: "abc123".into(),
        output: String::new(),
    };
    assert_eq!(StatusView::rebase(&status).height(42), 4);
    status.message = status.message.repeat(40);
    assert_eq!(
        StatusView::rebase(&status).height(42),
        STATUS_LINES as u16 + 2
    );
    for (width, height) in [(1, 1), (4, 4), (20, 6)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| StatusView::rebase(&status).render(frame, frame.area()))
            .unwrap();
    }
}

#[test]
fn path_prompt_shows_the_text_being_typed() {
    assert!(rendered(InsertPromptView::Typing("src/main.rs")).contains("src/main.rs"));
}

#[test]
fn grant_prompt_names_the_host_path() {
    let screen = rendered(InsertPromptView::Grant {
        host: "/host/private".into(),
        restarts: false,
    });
    assert!(screen.contains("/host/private"));
    assert!(!screen.contains("restarts"));
}

#[test]
fn grant_prompt_warns_when_mounting_restarts_the_interaction() {
    let screen = rendered(InsertPromptView::Grant {
        host: "/host/private".into(),
        restarts: true,
    });
    assert!(screen.contains("mounting restarts the interaction"));
}

#[test]
fn tag_filter_distinguishes_required_and_excluded_tags() {
    let available = vec!["bug".into(), "docs".into()];
    let selected = vec!["-bug".into(), "docs".into()];
    let screen = rendered_tags(&available, &selected);

    assert!(screen.contains("[-] bug"));
    assert!(screen.contains("[x] docs"));
    assert!(screen.contains("- exclude"));
}
