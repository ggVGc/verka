use super::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::path::PathBuf;
use styra_protocol::event::AgentEvent;

fn picker_summary(id: &str, selection: &str, age: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        name: None,
        first_prompt: None,
        tags: Vec::new(),
        workspace_id: "w-1".into(),
        path: std::path::PathBuf::from(id),
        selection: styra_protocol::agent::Selection::parse(selection).unwrap(),
        age: age.into(),
        created_at_ms: None,
        last_event_at_ms: None,
        last_event_age: String::new(),
        origin: None,
        branched_from: None,
        completed: styra_protocol::CompletionState::Active,
        integration: None,
    }
}

fn rendered_picker(sessions: &[SessionSummary], selected: usize) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| {
            render_picker(
                frame,
                "payments",
                sessions,
                selected,
                SessionOrder::LastActivity,
                Preview::Ready(&[]),
                None,
                false,
                false,
            )
        })
        .unwrap();
    screen_text(terminal.backend().buffer())
}

/// Split a flattened screen back into rows, so an assertion can say which
/// row a marker landed on rather than only that it is somewhere on screen.
fn screen_lines(screen: &str, width: usize) -> Vec<String> {
    screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|row| row.iter().collect())
        .collect()
}

fn screen_text(buffer: &ratatui::buffer::Buffer) -> String {
    buffer
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>()
}

#[test]
fn picker_lists_sessions_with_provider_and_age() {
    let sessions = vec![
        picker_summary("s-1", "codex", "2m ago"),
        picker_summary("s-2", "claude", "3h ago"),
    ];
    let screen = rendered_picker(&sessions, 0);
    assert!(screen.contains("payments · sessions"), "{screen}");
    assert!(screen.contains("codex"));
    assert!(screen.contains("2m ago"));
    assert!(screen.contains("s-1"));
    assert!(screen.contains("claude"));
    assert!(screen.contains("3h ago"));
    assert!(screen.contains("s-2"));
}

#[test]
fn a_branched_session_stays_on_one_compact_row() {
    let mut session = picker_summary("s-2", "claude", "2m ago");
    session.origin = Some(styra_protocol::SessionOrigin {
        session_id: "s-1".into(),
        provider: styra_protocol::agent::Provider::Codex,
        at_ms: None,
        history: styra_protocol::BranchHistory::ThroughSelected,
    });
    let screen = rendered_picker(&[session], 0);
    assert!(screen.contains("claude"), "{screen}");
    assert!(!screen.contains("converted from"), "{screen}");
}

#[test]
fn session_picker_indents_branched_sessions_as_a_tree() {
    let root = picker_summary("root", "codex", "3m ago");
    let mut branch = picker_summary("branch", "codex", "2m ago");
    branch.origin = Some(styra_protocol::SessionOrigin {
        session_id: "root".into(),
        provider: styra_protocol::agent::Provider::Codex,
        at_ms: Some(1000),
        history: styra_protocol::BranchHistory::ThroughSelected,
    });
    let sessions = vec![root, branch];

    let screen = rendered_picker(&sessions, 0);
    let branch_row = screen_lines(&screen, 80)
        .into_iter()
        .find(|line| line.contains("branch · codex"))
        .expect("branched session row");
    assert!(branch_row.starts_with("│  └─"), "{screen}");
}

#[test]
fn picker_prefers_a_session_name_without_repeating_its_identity() {
    let mut session = picker_summary("0000000123456-42-7", "codex", "2m ago");
    session.name = Some("Fix session picker".into());
    let screen = rendered_picker(&[session], 0);
    assert!(screen.contains("Fix session picker"), "{screen}");
    let list_pane = screen_lines(&screen, 80)
        .into_iter()
        .map(|line| line.chars().take(80 * 42 / 100).collect::<String>())
        .collect::<String>();
    assert!(!list_pane.contains("123456-42-7"), "{screen}");
}

#[test]
fn picker_shows_a_placeholder_when_there_are_no_sessions() {
    let screen = rendered_picker(&[], 0);
    assert!(screen.contains("no sessions found"));
}

#[test]
fn session_picker_previews_the_selected_sessions_log() {
    let sessions = vec![picker_summary("s-1", "codex", "2m ago")];
    let updates = vec![
        InteractionUpdate::Event(AgentEvent::CommandStarted {
            command: "cargo test".into(),
        }),
        InteractionUpdate::Event(AgentEvent::AgentMessage {
            text: "Tests pass.".into(),
        }),
    ];
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| {
            render_picker(
                frame,
                "styra",
                &sessions,
                0,
                SessionOrder::LastActivity,
                Preview::Ready(&updates),
                None,
                false,
                false,
            )
        })
        .unwrap();
    let screen = terminal
        .backend()
        .buffer()
        .clone()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(screen.contains("conversation · s-1"), "{screen}");
    assert!(screen.contains("Tests pass."), "{screen}");
    // Tool activity is not part of the conversation.
    assert!(!screen.contains("cargo test"), "{screen}");
}

fn workspace_summary(id: &str, name: &str, session_count: usize) -> WorkspaceSummary {
    WorkspaceSummary {
        id: id.into(),
        name: Some(name.into()),
        host_path: PathBuf::from(format!("/home/op/{id}")),
        git_repository: None,
        path: PathBuf::from(format!("/state/workspaces/{id}")),
        session_count,
        age: "2h ago".into(),
        created_at_ms: 1,
        last_accessed_at_ms: 1,
        launch: Default::default(),
    }
}

/// Wide enough that the left pane holds a whole row — name, counts,
/// liveness, age, host path — so an assertion about the row is not really
/// an assertion about where it was truncated.
const WORKSPACE_PICKER_WIDTH: usize = 160;

/// The Workspace list occupies the left half of the picker. The preview on
/// the right carries the selected Workspace's name in its own border title,
/// so a row has to be matched against this pane alone to be the list's.
fn workspace_rows(screen: &str) -> Vec<String> {
    let width = WORKSPACE_PICKER_WIDTH / 2;
    screen_lines(screen, WORKSPACE_PICKER_WIDTH)
        .into_iter()
        .map(|line| line.chars().take(width).collect())
        .collect()
}

fn rendered_workspace_picker(
    workspaces: &[WorkspaceSummary],
    selected: usize,
    interactions: &[InteractionSummary],
    preview: SessionsPreview<'_>,
) -> String {
    rendered_filtered_workspace_picker(workspaces, selected, interactions, preview, None)
}

fn rendered_filtered_workspace_picker(
    workspaces: &[WorkspaceSummary],
    selected: usize,
    interactions: &[InteractionSummary],
    preview: SessionsPreview<'_>,
    filter: Option<&str>,
) -> String {
    let mut terminal = Terminal::new(TestBackend::new(WORKSPACE_PICKER_WIDTH as u16, 14)).unwrap();
    terminal
        .draw(|frame| {
            render_workspace_picker(
                frame,
                workspaces,
                selected,
                interactions,
                preview,
                filter,
                false,
            )
        })
        .unwrap();
    screen_text(terminal.backend().buffer())
}

#[test]
fn deleted_workspace_picker_explains_restoration_and_handles_an_empty_list() {
    let workspaces = vec![workspace_summary("deleted", "archived work", 2)];
    let mut terminal = Terminal::new(TestBackend::new(180, 14)).unwrap();
    terminal
        .draw(|frame| {
            render_workspace_picker(
                frame,
                &workspaces,
                0,
                &[],
                SessionsPreview::Ready(&[]),
                None,
                true,
            )
        })
        .unwrap();
    let screen = screen_text(terminal.backend().buffer());
    assert!(screen.contains("Deleted Workspaces"), "{screen}");
    assert!(screen.contains("Ctrl+U restore"), "{screen}");
    assert!(screen.contains("archived work"), "{screen}");
    assert!(screen.contains("Restore this Workspace"), "{screen}");
    terminal
        .draw(|frame| {
            render_workspace_picker(frame, &[], 0, &[], SessionsPreview::Ready(&[]), None, true)
        })
        .unwrap();
    let screen = screen_text(terminal.backend().buffer());
    assert!(screen.contains("no deleted Workspaces"), "{screen}");
    assert!(screen.contains("Ctrl+T returns to active"), "{screen}");
}

#[test]
fn workspace_picker_lists_workspaces_before_sessions() {
    let mut workspace = workspace_summary("retry", "retry work", 3);
    workspace.host_path = PathBuf::from("/home/op/retry");
    let screen = rendered_workspace_picker(&[workspace], 0, &[], SessionsPreview::Ready(&[]));
    assert!(screen.contains("styra \u{b7} Workspaces"), "{screen}");
    assert!(screen.contains("retry work"), "{screen}");
    assert!(screen.contains("3 sessions"), "{screen}");
    assert!(screen.contains("/home/op/retry"), "{screen}");
}

#[test]
fn workspace_picker_says_when_the_filter_is_what_emptied_the_list() {
    let screen = rendered_filtered_workspace_picker(
        &[],
        0,
        &[],
        SessionsPreview::Ready(&[]),
        Some("nothing"),
    );

    assert!(screen.contains("filter: nothing"), "{screen}");
    assert!(screen.contains("no Workspaces match"), "{screen}");
}

#[test]
fn workspace_picker_marks_workspaces_holding_a_live_interaction() {
    let workspaces = vec![
        workspace_summary("w-1", "payments", 2),
        workspace_summary("w-2", "quiet", 1),
    ];
    let interactions = vec![
        interaction_summary("s-1", "codex", styra_protocol::InteractionActivity::Pending),
        interaction_summary(
            "s-2",
            "claude",
            styra_protocol::InteractionActivity::Pending,
        ),
        // Stopped: the server no longer accepts input, so it is not work
        // in flight and must not mark the Workspace.
        InteractionSummary {
            workspace_id: "w-2".into(),
            ..interaction_summary("s-3", "codex", styra_protocol::InteractionActivity::Stopped)
        },
    ];

    let screen =
        rendered_workspace_picker(&workspaces, 0, &interactions, SessionsPreview::Ready(&[]));

    let payments = workspace_rows(&screen)
        .into_iter()
        .find(|line| line.contains("payments"))
        .unwrap();
    let quiet = workspace_rows(&screen)
        .into_iter()
        .find(|line| line.contains("quiet"))
        .unwrap();
    assert!(payments.contains('\u{25cf}'), "{payments}");
    assert!(payments.contains("2 live"), "{payments}");
    assert!(!quiet.contains('\u{25cf}'), "{quiet}");
    assert!(!quiet.contains("live"), "{quiet}");
}

#[test]
fn workspace_picker_previews_the_selected_workspaces_sessions() {
    let workspaces = vec![
        workspace_summary("w-1", "payments", 2),
        workspace_summary("w-2", "quiet", 0),
    ];
    let mut named = picker_summary("s-1", "codex", "2m ago");
    named.name = Some("retry backoff".into());
    let sessions = vec![named, picker_summary("s-9", "claude", "1h ago")];

    let screen = rendered_workspace_picker(
        &workspaces,
        0,
        &[interaction_summary(
            "s-1",
            "codex",
            styra_protocol::InteractionActivity::Pending,
        )],
        SessionsPreview::Ready(&sessions),
    );

    assert!(screen.contains("sessions \u{b7} payments"), "{screen}");
    assert!(screen.contains("retry backoff"), "{screen}");
    assert!(screen.contains("2m ago"), "{screen}");
    assert!(screen.contains("1h ago"), "{screen}");
    // The live Session carries the same dot its Workspace row does.
    let live_row = screen_lines(&screen, WORKSPACE_PICKER_WIDTH)
        .into_iter()
        .find(|line| line.contains("retry backoff"))
        .unwrap();
    assert!(live_row.contains('\u{25cf}'), "{live_row}");
}

#[test]
fn an_unloaded_workspace_preview_reads_as_loading_rather_than_as_empty() {
    let workspaces = vec![workspace_summary("w-1", "payments", 2)];

    let loading = rendered_workspace_picker(&workspaces, 0, &[], SessionsPreview::Loading);
    assert!(loading.contains("loading"), "{loading}");

    let empty = rendered_workspace_picker(&workspaces, 0, &[], SessionsPreview::Ready(&[]));
    assert!(empty.contains("no sessions yet"), "{empty}");
}

/// Started in a directory no Workspace covers, Styra asks whether to
/// create one there in a popup over the list, rather than as a list row.
#[test]
fn the_workspace_offer_is_asked_over_the_list() {
    let workspaces = vec![workspace_summary("w-1", "payments", 2)];
    let mut terminal = Terminal::new(TestBackend::new(WORKSPACE_PICKER_WIDTH as u16, 14)).unwrap();
    terminal
        .draw(|frame| {
            render_workspace_picker(
                frame,
                &workspaces,
                0,
                &[],
                SessionsPreview::Ready(&[]),
                None,
                false,
            );
            render_workspace_offer(frame, Path::new("/home/op/fresh"));
        })
        .unwrap();
    let screen = screen_text(terminal.backend().buffer());

    assert!(screen.contains("No Workspace here"), "{screen}");
    assert!(screen.contains("Create a Workspace"), "{screen}");
    assert!(screen.contains("/home/op/fresh"), "{screen}");
    assert!(!screen.contains("+ new Workspace"), "{screen}");
    // It sits over the list, not across the Sessions pane beside it.
    for line in screen_lines(&screen, WORKSPACE_PICKER_WIDTH) {
        if let Some(column) = line.find("Create a Workspace") {
            assert!(column < WORKSPACE_PICKER_WIDTH / 2, "{screen}");
        }
    }
    // It opens near the top of the screen, whatever the terminal's height.
    assert!(
        screen_lines(&screen, WORKSPACE_PICKER_WIDTH)[2].contains("No Workspace here"),
        "{screen}"
    );
    // What is beneath it recedes, as it does behind the message box.
    let buffer = terminal.backend().buffer();
    assert_eq!(
        buffer[(WORKSPACE_PICKER_WIDTH as u16 - 2, 0)].fg,
        theme::MODAL_BACKDROP
    );
}

fn interaction_summary(
    id: &str,
    selection: &str,
    activity: styra_protocol::InteractionActivity,
) -> InteractionSummary {
    InteractionSummary {
        auto_retry: false,
        auto_commit: false,
        id: id.into(),
        name: None,
        tags: Vec::new(),
        workspace_id: "w-1".into(),
        selection: styra_protocol::agent::Selection::parse(selection).unwrap(),
        workspace: std::path::PathBuf::from("/home/op/project"),
        activity,
        activity_reason: None,
        activity_since_ms: 0,
        idle_unseen: false,
        interacted_at_ms: 0,
        last_user_message_at_ms: 0,
        uncommitted_changes: false,
        expected_branch: None,
        checkout: None,
        branched_from: None,
        last_message: None,
        recent_messages: Vec::new(),
        events: 0,
        completed: styra_protocol::CompletionState::Active,
        focus_requested: false,
        origin: None,
        integration: None,
        restored: false,
    }
}

#[test]
fn picker_highlights_the_selected_session() {
    let sessions = vec![
        picker_summary("s-1", "codex", "2m ago"),
        picker_summary("s-2", "codex", "3h ago"),
    ];

    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    terminal
        .draw(|frame| {
            render_picker(
                frame,
                "styra",
                &sessions,
                1,
                SessionOrder::LastActivity,
                Preview::Ready(&[]),
                None,
                false,
                false,
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();

    // Scan only the session pane: the log-preview pane's title spells out
    // the selected session's id too, so searching the full row width would
    // match that border row instead of the list row it is describing.
    let session_pane_width = buffer.area.width * 42 / 100;
    let row_containing = |text: &str| -> u16 {
        (0..buffer.area.height)
            .find(|&y| {
                let row: String = (0..session_pane_width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol())
                    .collect();
                row.contains(text)
            })
            .unwrap_or_else(|| panic!("no row contains {text:?}"))
    };
    let row_has_selection_backdrop = |y: u16| {
        (0..buffer.area.width)
            .any(|x| buffer.cell((x, y)).unwrap().style().bg == Some(theme::SELECTION_BACKGROUND))
    };

    assert!(!row_has_selection_backdrop(row_containing("s-1")));
    assert!(row_has_selection_backdrop(row_containing("s-2")));
}
