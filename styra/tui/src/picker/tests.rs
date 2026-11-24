use super::*;
use crate::keybindings::press;
use std::path::PathBuf;
use styra_protocol::{agent::Selection, InteractionActivity, SessionSummary};

fn session(id: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        name: None,
        first_prompt: None,
        tags: Vec::new(),
        workspace_id: "workspace".into(),
        path: PathBuf::from(id),
        selection: Selection::parse("codex").unwrap(),
        age: String::new(),
        created_at_ms: None,
        last_event_at_ms: None,
        last_event_age: String::new(),
        origin: None,
        branched_from: None,
        completed: CompletionState::Active,
        integration: None,
    }
}

fn interaction(id: &str, activity: InteractionActivity) -> InteractionSummary {
    InteractionSummary {
        auto_retry: false,
        auto_commit: false,
        id: id.into(),
        name: None,
        tags: Vec::new(),
        workspace_id: "workspace".into(),
        selection: Selection::parse("codex").unwrap(),
        workspace: PathBuf::from("/workspace"),
        activity,
        activity_reason: None,
        activity_since_ms: 0,
        idle_unseen: false,
        interacted_at_ms: 0,
        last_user_message_at_ms: 0,
        uncommitted_changes: false,
        expected_branch: None,
        checkout: None,
        last_message: None,
        recent_messages: Vec::new(),
        events: 0,
        branched_from: None,
        completed: CompletionState::Active,
        focus_requested: false,
        origin: None,
        integration: None,
        restored: false,
    }
}

fn interaction_in(
    id: &str,
    workspace_id: &str,
    activity: InteractionActivity,
) -> InteractionSummary {
    InteractionSummary {
        workspace_id: workspace_id.into(),
        ..interaction(id, activity)
    }
}

fn workspace(id: &str, last_accessed_at_ms: u64) -> WorkspaceSummary {
    WorkspaceSummary {
        id: id.into(),
        name: None,
        host_path: format!("/home/op/{id}").into(),
        git_repository: None,
        path: format!("/state/workspaces/{id}").into(),
        session_count: 0,
        age: "now".into(),
        created_at_ms: 1,
        last_accessed_at_ms,
        launch: Default::default(),
    }
}

fn template(name: &str) -> styra_protocol::TemplateSummary {
    styra_protocol::TemplateSummary {
        name: name.into(),
        description: String::new(),
    }
}

#[test]
fn template_picker_keeps_loading_and_choice_in_root_loop_state() {
    let mut picker = TemplatePicker::loading(
        1,
        "workspace".into(),
        LaunchScope::Workspace,
        vec!["rust".into(), "removed".into()],
    );
    assert_eq!(
        picker.handle_key(press(&keys::TEMPLATES_APPLY)),
        TemplatePickerAction::None
    );

    picker.loaded(vec![template("rust"), template("browser")]);
    assert_eq!(picker.chosen, vec!["rust"]);
    assert_eq!(
        picker.handle_key(press(&keys::TEMPLATES_APPLY)),
        TemplatePickerAction::Unchanged
    );
    picker.handle_key(press(&keys::TEMPLATES_NEXT));
    picker.handle_key(press(&keys::TEMPLATES_TOGGLE));
    assert_eq!(
        picker.handle_key(press(&keys::TEMPLATES_APPLY)),
        TemplatePickerAction::Apply {
            scope: LaunchScope::Workspace,
            chosen: vec!["rust".into(), "browser".into()],
        }
    );
}

#[test]
fn session_picker_opens_on_the_current_interaction_when_it_is_listed() {
    let sessions = vec![session("first"), session("current"), session("last")];
    assert_eq!(initial_session_selection(&sessions, Some("current")), 1);
    assert_eq!(initial_session_selection(&sessions, Some("gone")), 0);
}

#[test]
fn session_preview_reads_restored_interactions_from_the_stored_journal() {
    let live = interaction("live", InteractionActivity::Pending);
    let stopped_here = interaction("stopped-here", InteractionActivity::Stopped);
    let mut restored = interaction("restored", InteractionActivity::Stopped);
    restored.restored = true;
    let interactions = vec![live, stopped_here, restored];

    assert!(session_has_live_interaction("live", &interactions));
    assert!(session_has_live_interaction("stopped-here", &interactions));
    assert!(!session_has_live_interaction("restored", &interactions));
    assert!(!session_has_live_interaction("unknown", &interactions));
}

#[test]
fn session_picker_history_filter_shows_recent_or_all_sessions() {
    let now_ms = 10 * crate::session::RECENT_SESSION_WINDOW_MS;
    let mut fresh = session("fresh");
    fresh.last_event_at_ms = Some(now_ms - crate::session::RECENT_SESSION_WINDOW_MS);
    let mut old = session("old");
    old.last_event_at_ms = Some(now_ms - crate::session::RECENT_SESSION_WINDOW_MS - 1);
    let unknown_age = session("unknown-age");
    let sessions = vec![old, unknown_age, fresh];

    let recent = picker_sessions(
        &sessions,
        false,
        false,
        now_ms,
        SessionOrder::LastActivity,
        None,
    );
    assert_eq!(
        recent
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        ["fresh", "unknown-age"]
    );
    let all = picker_sessions(
        &sessions,
        true,
        false,
        now_ms,
        SessionOrder::LastActivity,
        None,
    );
    assert_eq!(all.len(), 3);
}

#[test]
fn session_picker_filter_matches_name_or_first_prompt_case_insensitively() {
    let mut named = session("named");
    named.name = Some("Payments migration".into());
    let mut prompted = session("prompted");
    prompted.first_prompt = Some("Investigate checkout timeout".into());
    let sessions = vec![named, prompted];

    for (query, expected) in [
        ("PAYMENTS", vec!["named"]),
        ("TIMEOUT", vec!["prompted"]),
        ("nonexistent", vec![]),
    ] {
        let matches = picker_sessions(
            &sessions,
            true,
            false,
            unix_now_ms(),
            SessionOrder::LastActivity,
            Some(query),
        );

        assert_eq!(
            matches
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "query: {query}"
        );
    }
}

#[test]
fn capital_j_and_k_jump_between_root_conversations() {
    let root = session("root");
    let mut branch = session("branch");
    branch.origin = Some(styra_protocol::SessionOrigin {
        session_id: "root".into(),
        provider: styra_protocol::agent::Provider::Codex,
        at_ms: Some(1),
        history: styra_protocol::BranchHistory::ThroughSelected,
    });
    let other_root = session("other-root");
    let sessions = vec![root, branch, other_root];

    assert_eq!(next_top_level_session(&sessions, 0), Some(2));
    assert_eq!(next_top_level_session(&sessions, 1), Some(2));
    assert_eq!(previous_top_level_session(&sessions, 1), Some(0));
    assert_eq!(previous_top_level_session(&sessions, 2), Some(0));
}

#[test]
fn workspace_filter_matches_the_name_and_host_path_case_insensitively() {
    let mut named = workspace("w-1", 3);
    named.name = Some("Payments API".into());
    let workspaces = vec![named, workspace("billing", 2), workspace("quiet", 1)];

    let by_name = picker_workspaces(&workspaces, Some("payments"));
    assert_eq!(
        by_name.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        vec!["w-1"]
    );

    // An unnamed Workspace is matched on the host directory name its row
    // shows in place of a name.
    let by_directory = picker_workspaces(&workspaces, Some("bill"));
    assert_eq!(
        by_directory
            .iter()
            .map(|w| w.id.as_str())
            .collect::<Vec<_>>(),
        vec!["billing"]
    );

    // The host path beside the name is searched too, so a named
    // Workspace is found by its directory as well.
    assert_eq!(picker_workspaces(&workspaces, Some("/HOME/OP")).len(), 3);
    let by_path = picker_workspaces(&workspaces, Some("op/w-1"));
    assert_eq!(
        by_path.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        vec!["w-1"]
    );
}

#[test]
fn workspace_and_session_filters_accept_noncontiguous_queries() {
    let mut named = workspace("billing", 1);
    named.name = Some("Payments API".into());
    assert_eq!(
        picker_workspaces(&[named.clone()], Some("PMTS")),
        [named.clone()]
    );
    assert_eq!(
        picker_workspaces(&[named.clone()], Some("/HM/BLG")),
        [named]
    );

    let mut session = session("migration");
    session.first_prompt = Some("Investigate checkout timeout".into());
    let matches = picker_sessions(
        &[session.clone()],
        true,
        false,
        unix_now_ms(),
        SessionOrder::LastActivity,
        Some("IVGT"),
    );
    assert_eq!(matches, [session]);
}

#[test]
fn workspace_filter_keeps_the_order_it_narrows() {
    let workspaces = vec![workspace("work-a", 3), workspace("work-b", 2)];

    let filtered = picker_workspaces(&workspaces, Some("work"));

    assert_eq!(
        filtered.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
        vec!["work-a", "work-b"]
    );
}

#[test]
fn empty_workspace_filter_shows_every_workspace() {
    let workspaces = vec![workspace("w-1", 2), workspace("w-2", 1)];

    assert_eq!(picker_workspaces(&workspaces, Some("")).len(), 2);
    assert_eq!(picker_workspaces(&workspaces, None).len(), 2);
}

#[test]
fn workspaces_sort_by_recent_access() {
    let mut workspaces = vec![
        workspace("older", 10),
        workspace("newest", 30),
        workspace("newer", 20),
    ];

    sort_workspaces(&mut workspaces, &[]);

    assert_eq!(
        workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>(),
        ["newest", "newer", "older"]
    );
}

#[test]
fn workspaces_with_live_interactions_sort_above_more_recently_accessed_ones() {
    let mut workspaces = vec![
        workspace("untouched", 40),
        workspace("running", 10),
        workspace("stopped", 30),
        workspace("idle", 20),
    ];
    let interactions = vec![
        interaction_in("a", "running", InteractionActivity::Running),
        interaction_in("b", "stopped", InteractionActivity::Stopped),
        interaction_in("c", "idle", InteractionActivity::Pending),
    ];

    sort_workspaces(&mut workspaces, &interactions);

    // Idle and running lead, ordered by access between themselves; a
    // Workspace whose only interaction has stopped ranks with the rest.
    assert_eq!(
        workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>(),
        ["idle", "running", "untouched", "stopped"]
    );
}

/// What orders the list is when work last happened in a Workspace, not
/// when the operator last opened one: a Workspace whose agent said
/// something an hour after it was last opened has to lead the one that was
/// opened since and then left alone.
#[test]
fn workspaces_sort_by_their_most_recent_interaction() {
    let mut workspaces = vec![
        workspace("opened-since", 500),
        workspace("worked-in", 100),
        workspace("never-touched", 300),
    ];
    let interactions = vec![
        InteractionSummary {
            activity_since_ms: 900,
            ..interaction_in("recent", "worked-in", InteractionActivity::Stopped)
        },
        // The Workspace is dated by its *most recent* interaction, so an
        // older sibling does not drag it back down the list.
        InteractionSummary {
            activity_since_ms: 200,
            ..interaction_in("stale", "worked-in", InteractionActivity::Stopped)
        },
        // A server too old to date its interactions says nothing about
        // when this Workspace was worked in; its access still does.
        InteractionSummary {
            activity_since_ms: 0,
            ..interaction_in("undated", "opened-since", InteractionActivity::Stopped)
        },
    ];

    sort_workspaces(&mut workspaces, &interactions);

    assert_eq!(
        workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<Vec<_>>(),
        ["worked-in", "opened-since", "never-touched"]
    );
}

/// The age column is read against the ordering beside it, so it has to
/// date a Workspace the same way the ordering does: by its most recent
/// interaction, not by when it was created.
#[test]
fn the_age_column_says_how_long_since_work_last_happened() {
    let hour_ms = 3_600_000;
    let mut workspaces = vec![
        WorkspaceSummary {
            created_at_ms: 1,
            ..workspace("worked-in", hour_ms)
        },
        // No interaction dates this one, so its recorded access is all
        // there is to say when it was last used.
        WorkspaceSummary {
            created_at_ms: 1,
            ..workspace("only-opened", 20 * hour_ms)
        },
    ];
    let interactions = vec![InteractionSummary {
        activity_since_ms: 23 * hour_ms,
        ..interaction_in("recent", "worked-in", InteractionActivity::Stopped)
    }];

    label_recency(&mut workspaces, &interactions, 24 * hour_ms);

    assert_eq!(workspaces[0].age, "1h ago");
    assert_eq!(workspaces[1].age, "4h ago");
}

/// A Workspace worked in seconds ago should not read "0m ago", and one
/// dated ahead of this client's clock should not read as a negative age.
#[test]
fn work_just_done_and_work_dated_ahead_both_read_as_just_now() {
    let mut workspaces = vec![workspace("fresh", 9_000), workspace("ahead", 60_000)];

    label_recency(&mut workspaces, &[], 10_000);

    assert_eq!(workspaces[0].age, "just now");
    assert_eq!(workspaces[1].age, "just now");
}
