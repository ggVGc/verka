use super::*;
use crate::activity::RateLimit;
use styra_protocol::agent::{Provider, Selection};
use styra_protocol::{Direction, RawLine};

#[test]
fn submission_failures_are_visible_and_keep_the_full_error_in_the_log() {
    use crate::activity::IdleReason;
    use styra_protocol::{LogLevel, Request, Response};

    struct FailingServer(&'static str);
    impl styra_server::InProcessServer for FailingServer {
        fn handle(&self, request: Request) -> anyhow::Result<Response> {
            let operation = match request {
                Request::CreateSession(_) => "create",
                Request::ResumeSession(_) => "resume",
                Request::SendMessage { .. } => "send",
                Request::QueueMessage { .. } => "queue",
                _ => panic!("unexpected request: {request:?}"),
            };
            assert_eq!(operation, self.0);
            Err(
                anyhow::anyhow!("fatal: unable to generate diff for all.csv")
                    .context("snapshot failed"),
            )
        }

        fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    for (operation, session_id, status, mut live) in [
        ("create", "", Status::Pending, Attachment::Detached),
        (
            "resume",
            "session",
            Status::Stopped(StopReason::Paused),
            Attachment::Detached,
        ),
        (
            "resume",
            "session",
            Status::Stopped(StopReason::Paused),
            Attachment::Attached { cursor: 0 },
        ),
        (
            "send",
            "session",
            Status::Idle(IdleReason::TurnComplete),
            Attachment::Attached { cursor: 0 },
        ),
        (
            "queue",
            "session",
            Status::Running,
            Attachment::Attached { cursor: 0 },
        ),
    ] {
        let client = Client::in_process(std::sync::Arc::new(FailingServer(operation)));
        let mut app = App::new(Selection::new(Provider::Codex), session_id);
        app.activity.status = status;

        submit_message(
            &mut app,
            &client,
            "workspace",
            &mut live,
            "hello".into(),
            false,
        );

        let notices = app.notices.iter().collect::<Vec<_>>();
        assert_eq!(notices.len(), 1, "{operation}");
        assert_eq!(notices[0].level, LogLevel::Error);
        assert!(notices[0].text.contains("snapshot failed"));
        assert!(notices[0]
            .text
            .contains("fatal: unable to generate diff for all.csv"));
        let entries = app.log.iter().collect::<Vec<_>>();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].level, LogLevel::Error);
        assert_eq!(entries[0].message, notices[0].text);
        if matches!(operation, "create" | "resume") {
            assert_eq!(app.composer.message(), "hello");
        }
    }
}

#[test]
fn replayed_diagnostic_errors_do_not_raise_new_notices() {
    let mut app = App::new(Selection::new(Provider::Codex), "session");
    apply_update(
        &mut app,
        InteractionUpdate::Log(LogEntry::error("old failure")),
    );
    assert!(app.notices.is_empty());
    assert_eq!(app.log.iter().count(), 1);
}

fn workspace(id: &str, host_path: &str) -> WorkspaceSummary {
    WorkspaceSummary {
        id: id.into(),
        name: None,
        host_path: host_path.into(),
        git_repository: None,
        path: format!("/state/workspaces/{id}").into(),
        session_count: 0,
        age: "now".into(),
        created_at_ms: 1,
        last_accessed_at_ms: 1,
        launch: Default::default(),
    }
}

fn summary(id: &str, created_at_ms: u64, last_event_at_ms: Option<u64>) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        name: None,
        first_prompt: None,
        tags: Vec::new(),
        workspace_id: "w-1".into(),
        path: PathBuf::from(id),
        selection: Selection::new(Provider::Codex),
        age: String::new(),
        created_at_ms: Some(created_at_ms),
        last_event_at_ms,
        last_event_age: String::new(),
        origin: None,
        branched_from: None,
        completed: CompletionState::Active,
        integration: None,
    }
}

/// Every way of switching Interactions carries what the operator put on
/// the screen being left, not only the navigator's.
#[test]
fn switching_carries_the_operators_state_and_the_standing_launch() {
    let selection = Selection::new(Provider::Codex);
    let mut app = App::new(selection.clone(), "session-1");
    app.outbox.set_contract(Some(Contract::Lines));
    app.recent_models = vec!["gpt-5.6-sol".into()];
    let mut live = Attachment::Detached;
    let standing = LaunchPolicy {
        templates: vec!["rust".into()],
        ..Default::default()
    };

    let next = App::new(selection, "session-2");
    let next_live = Attachment::Attached { cursor: 7 };
    switch_to(&mut app, &mut live, (next, next_live), &standing);

    assert_eq!(app.session_id, "session-2");
    assert!(matches!(live, Attachment::Attached { cursor: 7 }));
    assert_eq!(app.outbox.contract(), Some(Contract::Lines));
    assert_eq!(app.recent_models, vec!["gpt-5.6-sol".to_owned()]);
    assert_eq!(app.launch.interaction, standing);
}

#[test]
fn branch_dialog_starts_the_branch_without_leaving_the_source() {
    use crate::keybindings::{press, BRANCH_CHANGE, EDITOR_SEND, EDITOR_SEND_IN_BRANCH};
    use std::sync::Mutex;
    use styra_protocol::{BranchCheckout, BranchHistory, Request, Response};

    struct BranchServer {
        checkout: BranchCheckout,
        seen: Mutex<Vec<&'static str>>,
    }
    impl styra_server::InProcessServer for BranchServer {
        fn handle(&self, request: Request) -> anyhow::Result<Response> {
            match request {
                Request::BranchSession {
                    id,
                    at_ms,
                    history,
                    provider,
                    selection,
                    checkout,
                } => {
                    assert_eq!(id, "source");
                    assert_eq!(at_ms, Some(42));
                    assert_eq!(history, BranchHistory::SelectedOnly);
                    assert_eq!(checkout, Some(self.checkout));
                    let selection = selection.unwrap();
                    assert_eq!(provider, Some(selection.provider));
                    assert_eq!(selection.provider, Provider::Claude);
                    self.seen.lock().unwrap().push("branch");
                    Ok(Response::SessionBranched(summary("branched", 1, None)))
                }
                Request::RenameSession(rename) => {
                    assert_eq!(rename.id, "branched");
                    assert_eq!(rename.name.as_deref(), Some("branch instruction"));
                    self.seen.lock().unwrap().push("rename");
                    let mut renamed = summary("branched", 1, None);
                    renamed.name = rename.name;
                    Ok(Response::SessionRenamed(renamed))
                }
                Request::ResumeSession(resume) => {
                    assert_eq!(resume.id, "branched");
                    let selection = resume.selection.unwrap();
                    assert_eq!(selection.provider, Provider::Claude);
                    self.seen.lock().unwrap().push("resume");
                    Ok(Response::SessionResumed(SessionInfo {
                        id: resume.id,
                        name: None,
                        workspace_id: "workspace".into(),
                        selection,
                        workspace: "/workspace".into(),
                        journal_path: "/journal".into(),
                        driva: Default::default(),
                        updates_after: 0,
                        queued: Vec::new(),
                    }))
                }
                Request::SendMessage { id, message } => {
                    assert_eq!(id, "branched");
                    assert_eq!(message.text, "branch instruction");
                    assert_eq!(message.contract, Some(Contract::Lines));
                    self.seen.lock().unwrap().push("send");
                    Ok(Response::Accepted)
                }
                _ => anyhow::bail!("unexpected request"),
            }
        }
        fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    for (send, checkout) in [
        (&EDITOR_SEND, BranchCheckout::ShareSource),
        (&EDITOR_SEND_IN_BRANCH, BranchCheckout::NewWorktree),
    ] {
        let server = std::sync::Arc::new(BranchServer {
            checkout,
            seen: Mutex::default(),
        });
        let client = Client::in_process(server.clone());
        let mut app = App::new(Selection::new(Provider::Codex), "source");
        app.composer.set("source draft".into());
        let mut prompt = crate::branch::BranchPrompt::new(42, Selection::new(Provider::Claude));
        prompt.composer.set("branch instruction".into());
        prompt.contract = Some(Contract::Lines);
        app.branch_prompt = Some(prompt);
        crate::input::handle_branch_prompt_key(&mut app, &client, press(&BRANCH_CHANGE));
        crate::input::handle_branch_prompt_key(&mut app, &client, press(send));
        assert!(app.branch_prompt.is_none());
        assert_eq!(app.session_id, "source");
        assert_eq!(app.composer.message(), "source draft");
        assert_eq!(app.selection.provider, Provider::Codex);
        assert!(app.take_request().is_none());
        assert_eq!(
            *server.seen.lock().unwrap(),
            ["branch", "rename", "resume", "send"]
        );
        assert_eq!(
            app.interactions.name("branched"),
            Some("branch instruction")
        );
    }
}

/// A message started from the diff and carried over to a branch with
/// ctrl-b is still about the diff's file and line when the branch sends it.
#[test]
fn a_message_carried_to_a_branch_keeps_its_diff_source() {
    use crate::keybindings::{press, EDITOR_BRANCH, EDITOR_SEND};
    use std::sync::Mutex;
    use styra_protocol::{Request, Response};

    #[derive(Default)]
    struct SentServer {
        sent: Mutex<Vec<String>>,
    }
    impl styra_server::InProcessServer for SentServer {
        fn handle(&self, request: Request) -> anyhow::Result<Response> {
            match request {
                Request::BranchSession { .. } => {
                    Ok(Response::SessionBranched(summary("branched", 1, None)))
                }
                Request::RenameSession(rename) => {
                    let mut renamed = summary("branched", 1, None);
                    renamed.name = rename.name;
                    Ok(Response::SessionRenamed(renamed))
                }
                Request::ResumeSession(resume) => Ok(Response::SessionResumed(SessionInfo {
                    id: resume.id,
                    name: None,
                    workspace_id: "workspace".into(),
                    selection: resume.selection.unwrap(),
                    workspace: "/workspace".into(),
                    journal_path: "/journal".into(),
                    driva: Default::default(),
                    updates_after: 0,
                    queued: Vec::new(),
                })),
                Request::SendMessage { message, .. } => {
                    self.sent.lock().unwrap().push(message.text);
                    Ok(Response::Accepted)
                }
                _ => anyhow::bail!("unexpected request"),
            }
        }
        fn shutdown(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let server = std::sync::Arc::new(SentServer::default());
    let client = Client::in_process(server.clone());
    let mut app = App::new(Selection::new(Provider::Codex), "source");
    apply_update(
        &mut app,
        styra_protocol::InteractionUpdate::Event(styra_protocol::event::AgentEvent::UserMessage {
            text: "question".into(),
        }),
    );
    apply_update(
        &mut app,
        styra_protocol::InteractionUpdate::Raw(styra_protocol::RawLine {
            direction: styra_protocol::Direction::ToAgent,
            text: "question".into(),
            at_ms: 42,
        }),
    );
    app.enter_input();
    app.message_source = Some("b.txt:5".into());
    app.composer.set("look here".into());

    crate::input::handle_input_key(
        &mut app,
        &client,
        "workspace",
        &mut Attachment::Detached,
        press(&EDITOR_BRANCH),
    );
    assert!(app.branch_prompt.is_some());
    crate::input::handle_branch_prompt_key(&mut app, &client, press(&EDITOR_SEND));

    assert!(app.branch_prompt.is_none());
    assert_eq!(
        *server.sent.lock().unwrap(),
        ["look here\n\nSource: b.txt:5"]
    );
    assert_eq!(app.message_source, None);
}

#[test]
fn sessions_order_by_last_activity_or_by_creation() {
    let mut sessions = vec![
        summary("old-but-active", 100, Some(900)),
        summary("new-and-quiet", 300, Some(300)),
        summary("never-ran", 200, None),
    ];

    sort_sessions(&mut sessions, SessionOrder::LastActivity);
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["old-but-active", "new-and-quiet", "never-ran"]);

    sort_sessions(&mut sessions, SessionOrder::Created);
    let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["new-and-quiet", "never-ran", "old-but-active"]);
}

#[test]
fn session_picker_tree_sorts_roots_and_each_branch_level_independently() {
    let newest_root = summary("newest-root", 400, Some(400));
    let older_root = summary("older-root", 100, Some(100));
    let mut older_child = summary("older-child", 200, Some(200));
    older_child.origin = Some(styra_protocol::SessionOrigin {
        session_id: "older-root".into(),
        provider: Provider::Codex,
        at_ms: Some(1),
        history: styra_protocol::BranchHistory::ThroughSelected,
    });
    let mut newer_child = summary("newer-child", 150, Some(900));
    newer_child.origin = Some(styra_protocol::SessionOrigin {
        session_id: "older-root".into(),
        provider: Provider::Codex,
        at_ms: Some(2),
        history: styra_protocol::BranchHistory::ThroughSelected,
    });
    let mut grandchild = summary("grandchild", 500, Some(800));
    grandchild.origin = Some(styra_protocol::SessionOrigin {
        session_id: "older-child".into(),
        provider: Provider::Codex,
        at_ms: Some(3),
        history: styra_protocol::BranchHistory::ThroughSelected,
    });

    let mut sessions = vec![
        older_child,
        newest_root.clone(),
        grandchild,
        older_root.clone(),
        newer_child,
    ];
    sort_sessions_tree(&mut sessions, SessionOrder::LastActivity);

    let ids = sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    // Roots retain the normal last-activity ordering. Within older-root,
    // the newer child comes first; its sibling's own child follows it.
    assert_eq!(
        ids,
        [
            "newest-root",
            "older-root",
            "newer-child",
            "older-child",
            "grandchild"
        ]
    );
    assert_eq!(session_tree_depths(&sessions), [0, 0, 1, 1, 2]);

    // Changing sort order reorders the siblings too, while leaving the
    // branch beneath its parent rather than returning to a flat list.
    sort_sessions_tree(&mut sessions, SessionOrder::Created);
    let ids = sessions
        .iter()
        .map(|session| session.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "newest-root",
            "older-root",
            "older-child",
            "grandchild",
            "newer-child"
        ]
    );
}

#[test]
fn messages_include_all_incoming_traffic_in_live_and_saved_sessions() {
    let outgoing = RawLine {
        direction: Direction::ToAgent,
        text: "question".into(),
        at_ms: 1,
    };
    let incoming = RawLine {
        direction: Direction::FromAgent,
        text: r#"{"method":"control"}"#.into(),
        at_ms: 2,
    };
    let events = vec![
        styra_protocol::event::AgentEvent::UserMessage {
            text: "question".into(),
        },
        styra_protocol::event::AgentEvent::Unknown {
            wire_type: "control".into(),
        },
    ];
    let mut live = App::new(Selection::new(Provider::Codex), "live");
    for line in [outgoing.clone(), incoming.clone()] {
        apply_update(&mut live, InteractionUpdate::Raw(line));
    }
    let mut saved = App::new(Selection::new(Provider::Codex), "saved");
    replay_into(&mut saved, events, vec![outgoing, incoming.clone()]);
    for app in [&live, &saved] {
        assert_eq!(app.messages.as_slice(), &[incoming.clone()]);
        assert_eq!(app.raw.as_slice().len(), 2);
    }
    assert!(App::new(Selection::new(Provider::Codex), "other")
        .messages
        .is_empty());
}

#[test]
fn an_outgoing_wire_line_becomes_the_live_user_entrys_branch_point() {
    let mut app = App::new(Selection::new(Provider::Codex), "s-1");
    apply_update(
        &mut app,
        InteractionUpdate::Event(styra_protocol::event::AgentEvent::UserMessage {
            text: "question".into(),
        }),
    );
    apply_update(
        &mut app,
        InteractionUpdate::Raw(RawLine {
            direction: Direction::ToAgent,
            text: "question".into(),
            at_ms: 42,
        }),
    );

    open_branch_prompt(&mut app);
    assert_eq!(app.branch_prompt.as_ref().unwrap().at_ms(), 42);
}

/// A branched Session's journal opens with the host-side branch marker
/// written before its copied history. `journal::replay` turns that record
/// into a `Branched` event, but `journal::replay_raw` skips it — it never
/// crossed the wire — so the events list is one longer than the raw one.
/// Replaying them in lockstep must still pair each entry with its own wire
/// line, or the branch loses exactly the entry it was taken on: the last
/// one.
#[test]
fn a_branched_journal_replays_its_whole_history() {
    use styra_protocol::event::{AgentEvent, BranchDirection};

    let mut app = App::new(Selection::new(Provider::Codex), "s-2");
    let agent_line = r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"answer"}}}"#;
    replay_into(
        &mut app,
        vec![
            // Written by `Journal::record_branch`; no wire line of its own.
            AgentEvent::Branched {
                direction: BranchDirection::From,
                session: "s-1".into(),
                name: None,
            },
            AgentEvent::UserMessage {
                text: "question".into(),
            },
            AgentEvent::AgentMessage {
                text: "answer".into(),
            },
        ],
        vec![
            RawLine {
                direction: Direction::ToAgent,
                text: "question".into(),
                at_ms: 10,
            },
            RawLine {
                direction: Direction::FromAgent,
                text: agent_line.into(),
                at_ms: 20,
            },
        ],
    );

    assert_eq!(
        app.timeline
            .entries
            .iter()
            .map(|entry| entry.event().clone())
            .collect::<Vec<_>>(),
        vec![
            AgentEvent::Branched {
                direction: BranchDirection::From,
                session: "s-1".into(),
                name: None,
            },
            AgentEvent::UserMessage {
                text: "question".into(),
            },
            AgentEvent::AgentMessage {
                text: "answer".into(),
            },
        ],
        "the marker carries no wire line, so it must not consume the first one"
    );

    // And each entry's branch point is its own wire line, not the one
    // before it: branching the replayed Session again has to cut where the
    // operator is looking.
    app.select_last();
    open_branch_prompt(&mut app);
    assert_eq!(app.branch_prompt.as_ref().unwrap().at_ms(), 20);
}

/// The same for a model change, the other record the host writes in its
/// own right: it sits mid-history, so a replay that let it eat a wire line
/// would mispair only the turns after it — the quieter half of the same
/// defect.
#[test]
fn a_replayed_model_change_does_not_shift_the_turns_after_it() {
    use styra_protocol::event::AgentEvent;

    let mut app = App::new(Selection::new(Provider::Codex), "s-1");
    replay_into(
        &mut app,
        vec![
            AgentEvent::UserMessage {
                text: "first".into(),
            },
            AgentEvent::ModelChanged {
                model: Some("gpt-5.6-luna".into()),
                effort: None,
            },
            AgentEvent::UserMessage {
                text: "second".into(),
            },
        ],
        vec![
            RawLine {
                direction: Direction::ToAgent,
                text: "first".into(),
                at_ms: 10,
            },
            RawLine {
                direction: Direction::ToAgent,
                text: "second".into(),
                at_ms: 20,
            },
        ],
    );

    assert_eq!(app.timeline.entries.len(), 3);
    app.select_last();
    open_branch_prompt(&mut app);
    assert_eq!(app.branch_prompt.as_ref().unwrap().at_ms(), 20);
}

#[test]
fn a_branch_marker_is_followed_to_the_session_it_names() {
    let mut app = App::new(Selection::new(Provider::Codex), "s-1");
    apply_update(
        &mut app,
        InteractionUpdate::Event(styra_protocol::event::AgentEvent::Branched {
            direction: styra_protocol::event::BranchDirection::To,
            session: "s-2".into(),
            name: None,
        }),
    );
    app.select_last();

    follow_branch(&mut app);
    assert_eq!(
        app.requests.front(),
        Some(&crate::app::Request::OpenSession("s-2".into()))
    );
}

/// Any other entry has nowhere to jump to, so following says so rather
/// than replacing the screen with an unrelated Session.
#[test]
fn following_a_plain_entry_opens_nothing() {
    let mut app = App::new(Selection::new(Provider::Codex), "s-1");
    apply_update(
        &mut app,
        InteractionUpdate::Event(styra_protocol::event::AgentEvent::UserMessage {
            text: "question".into(),
        }),
    );
    app.select_last();

    follow_branch(&mut app);
    assert!(app.requests.is_empty());
}

#[test]
fn toggling_the_order_returns_to_where_it_started() {
    assert_eq!(SessionOrder::LastActivity.toggled(), SessionOrder::Created);
    assert_eq!(
        SessionOrder::LastActivity.toggled().toggled(),
        SessionOrder::LastActivity
    );
}

fn app() -> App {
    App::new(
        Selection {
            provider: Provider::Codex,
            model: Provider::Codex.default_model().to_owned(),
            effort: Provider::Codex.default_effort(),
        },
        "session-1",
    )
}

#[test]
fn stopped_session_is_viewed_until_its_next_message_resumes_it() {
    let mut app = app();
    let mut live = Attachment::Attached { cursor: 7 };

    mark_stopped(&mut app, &mut live, StopReason::Paused);

    assert_eq!(app.session_id, "session-1");
    assert_eq!(app.activity.status, Status::Stopped(StopReason::Paused));
    assert_eq!(live, Attachment::Detached);
}

/// Pausing an interaction a spent window has already halted is finishing
/// what the window started. The window is the part the operator cannot
/// see, so it is what the stopped status says.
#[test]
fn a_stop_under_a_spent_window_names_the_window_rather_than_the_key() {
    let mut app = app();
    let mut live = Attachment::Attached { cursor: 7 };
    app.activity.note_refused(RateLimit {
        window: "five_hour".into(),
        resets_at_ms: Some(2_000),
    });

    mark_stopped(&mut app, &mut live, StopReason::Paused);

    assert_eq!(
        app.activity.status,
        Status::Stopped(StopReason::RateLimited(RateLimit {
            window: "five_hour".into(),
            resets_at_ms: Some(2_000),
        }))
    );
}

#[test]
fn opening_the_same_directory_reuses_its_workspace_even_through_a_symlink() {
    struct WorkspaceServer(PathBuf);
    impl styra_server::InProcessServer for WorkspaceServer {
        fn handle(&self, request: styra_protocol::Request) -> Result<styra_protocol::Response> {
            use styra_protocol::{Request, Response};
            use styra_server::workspace;
            Ok(match request {
                Request::ListWorkspaces => Response::Workspaces(workspace::list(&self.0)?),
                Request::Workspace { id } => Response::Workspace(workspace::get(&self.0, &id)?),
                Request::CreateWorkspace(request) => Response::WorkspaceCreated(workspace::create(
                    &self.0,
                    &request.host_path,
                    request.name,
                )?),
                _ => anyhow::bail!("unexpected request"),
            })
        }

        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    let root = tempfile::tempdir().unwrap();
    let host = root.path().join("host");
    std::fs::create_dir(&host).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&host, &alias).unwrap();
    let client = Client::in_process(std::sync::Arc::new(WorkspaceServer(
        root.path().join("store"),
    )));

    let created = workspace_for_host(&client, &host).unwrap();
    let reopened = workspace_for_host(&client, &alias).unwrap();

    assert_eq!(reopened.id, created.id);
    assert_eq!(client.list_workspaces().unwrap().len(), 1);
}

#[test]
fn finds_the_workspace_associated_with_the_host_directory() {
    let workspaces = vec![
        workspace("w-other", "/home/op/other"),
        workspace("w-project", "/home/op/project"),
    ];

    let found = find_workspace_for_host(&workspaces, Path::new("/home/op/project")).unwrap();

    assert_eq!(found.id, "w-project");
}

#[test]
fn does_not_associate_a_different_directory() {
    let workspaces = vec![workspace("w-project", "/home/op/project")];

    assert!(find_workspace_for_host(&workspaces, Path::new("/home/op/elsewhere")).is_none());
}
