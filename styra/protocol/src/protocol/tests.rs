use super::*;

#[test]
fn simple_requests_preserve_their_wire_fields_and_round_trip() {
    for (request, fields) in [
        (
            Request::RenameWorkspace(RenameWorkspace {
                id: "workspace-1".into(),
                name: Some("payments".into()),
            }),
            vec![
                ("/operation", serde_json::json!("rename_workspace")),
                ("/data/id", serde_json::json!("workspace-1")),
                ("/data/name", serde_json::json!("payments")),
            ],
        ),
        (
            Request::ProviderRaw { id: "s-1".into() },
            vec![
                ("/operation", serde_json::json!("provider_raw")),
                ("/data/id", serde_json::json!("s-1")),
            ],
        ),
        (
            Request::CreateSession(CreateSession {
                workspace_id: "w-1".into(),
                selection: crate::agent::Selection {
                    provider: crate::agent::Provider::Claude,
                    model: "claude-opus-5".into(),
                    effort: crate::agent::Effort::XHigh,
                },
                launch: LaunchPolicy::default(),
                create_worktree: false,
                checkout_from: None,
                base_branch: None,
                message: None,
                name: None,
                contract: None,
                focus: false,
            }),
            vec![
                ("/data/selection/provider", serde_json::json!("claude")),
                ("/data/selection/model", serde_json::json!("claude-opus-5")),
                ("/data/selection/effort", serde_json::json!("xhigh")),
            ],
        ),
        (
            Request::CreateSessionWorktree {
                id: "styra-1".into(),
            },
            vec![
                ("/operation", serde_json::json!("create_session_worktree")),
                ("/data/id", serde_json::json!("styra-1")),
            ],
        ),
        (
            Request::SetWorkspaceHostPath {
                workspace_id: "w-1".into(),
                host_path: PathBuf::from("/srv/elsewhere"),
            },
            vec![
                ("/operation", serde_json::json!("set_workspace_host_path")),
                ("/data/workspace_id", serde_json::json!("w-1")),
                ("/data/host_path", serde_json::json!("/srv/elsewhere")),
            ],
        ),
        (
            Request::ConvertSessionProvider {
                id: "styra-1".into(),
            },
            vec![
                ("/operation", serde_json::json!("convert_session_provider")),
                ("/data/id", serde_json::json!("styra-1")),
            ],
        ),
        (
            Request::RenameSession(RenameSession {
                id: "styra-1".into(),
                name: Some("Fix session picker".into()),
            }),
            vec![
                ("/operation", serde_json::json!("rename_session")),
                ("/data/id", serde_json::json!("styra-1")),
                ("/data/name", serde_json::json!("Fix session picker")),
            ],
        ),
        (
            Request::SendMessage {
                id: "styra-1".into(),
                // The CLI uses this constructor for ordinary messages.
                message: SendMessage::new("hello"),
            },
            vec![
                ("/operation", serde_json::json!("send_message")),
                ("/data/id", serde_json::json!("styra-1")),
                ("/data/message/text", serde_json::json!("hello")),
            ],
        ),
    ] {
        let json = serde_json::to_value(&request).unwrap();
        for (pointer, expected) in fields {
            assert_eq!(
                json.pointer(pointer),
                Some(&expected),
                "{request:?}: {pointer}"
            );
        }
        assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    }
}

#[test]
fn update_stream_has_an_explicit_cursor_and_tagged_payload() {
    let response = Updates {
        updates: vec![SequencedUpdate {
            sequence: 4,
            update: InteractionUpdate::Log(LogEntry::info("ready")),
        }],
        next: 4,
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["next"], 4);
    assert_eq!(json["updates"][0]["sequence"], 4);
    assert_eq!(json["updates"][0]["update"]["type"], "log");
    assert_eq!(json["updates"][0]["update"]["data"]["message"], "ready");
    assert_eq!(serde_json::from_value::<Updates>(json).unwrap(), response);
}

#[test]
fn requests_are_self_describing_json_messages() {
    let request = Request::Updates {
        id: "s-1".into(),
        after: 8,
        raw: true,
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "updates");
    assert_eq!(json["data"]["id"], "s-1");
    assert_eq!(json["data"]["after"], 8);
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    // A client that predates the raw opt-out still gets raw lines.
    assert_eq!(
        serde_json::from_str::<Request>(r#"{"operation":"updates","data":{"id":"s-1","after":8}}"#)
            .unwrap(),
        request
    );
    assert!(
        serde_json::from_str::<Request>(r#"{"api_version":"v3","operation":"health"}"#).is_err()
    );
}

#[test]
fn listing_interactions_without_a_query_still_parses_and_lists_everything() {
    for json in [
        r#"{"operation":"list_interactions"}"#,
        r#"{"operation":"list_interactions","data":null}"#,
    ] {
        assert_eq!(
            serde_json::from_str::<Request>(json).unwrap(),
            Request::ListInteractions(None)
        );
    }
    let request = Request::ListInteractions(Some(InteractionQuery {
        workspace_id: Some("w-1".into()),
        skip_done: true,
        ..InteractionQuery::default()
    }));
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(
        json["data"],
        serde_json::json!({"workspace_id": "w-1", "skip_done": true})
    );
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    assert!(serde_json::from_str::<Request>(
        r#"{"operation":"list_interactions","data":{"skip_finished":true}}"#
    )
    .is_err());
}

#[test]
fn an_interaction_query_narrows_and_keep_overrides_only_the_skips() {
    use InteractionActivity::{Pending, Stopped};
    let all = InteractionQuery::default();
    assert!(all.admits("s-1", "w-1", Stopped, CompletionState::Sealed));

    let unfinished = InteractionQuery {
        skip_done: true,
        ..InteractionQuery::default()
    };
    assert!(unfinished.admits("s-1", "w-1", Stopped, CompletionState::Active));
    assert!(!unfinished.admits("s-1", "w-1", Stopped, CompletionState::Completed));

    let live = InteractionQuery {
        skip_stopped: true,
        ..InteractionQuery::default()
    };
    assert!(live.admits("s-1", "w-1", Pending, CompletionState::Active));
    assert!(!live.admits("s-1", "w-1", Stopped, CompletionState::Active));

    let on_screen = InteractionQuery {
        workspace_id: Some("w-1".into()),
        skip_done: true,
        skip_stopped: true,
        keep: vec!["s-1".into()],
        ..InteractionQuery::default()
    };
    assert!(on_screen.admits("s-1", "w-1", Stopped, CompletionState::Sealed));
    assert!(!on_screen.admits("s-2", "w-1", Stopped, CompletionState::Sealed));
    assert!(!on_screen.admits("s-1", "w-2", Pending, CompletionState::Active));

    let one = InteractionQuery {
        id: Some("s-1".into()),
        ..InteractionQuery::default()
    };
    assert!(one.admits("s-1", "w-1", Stopped, CompletionState::Active));
    assert!(!one.admits("s-2", "w-1", Pending, CompletionState::Active));
}

/// The scope is optional on the wire, so a client asking about every
/// Workspace sends the operation and nothing else.
#[test]
fn cleaning_worktrees_names_a_workspace_or_none() {
    let scoped = Request::CleanWorktrees {
        workspace_id: Some("workspace-1".into()),
    };
    let json = serde_json::to_value(&scoped).unwrap();
    assert_eq!(json["operation"], "clean_worktrees");
    assert_eq!(json["data"]["workspace_id"], "workspace-1");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), scoped);

    let every = Request::CleanWorktrees { workspace_id: None };
    assert_eq!(
        serde_json::from_str::<Request>(r#"{"operation":"clean_worktrees","data":{}}"#).unwrap(),
        every
    );

    let response = Response::WorktreesCleaned(vec![CleanedWorktree {
        session_id: "s-1".into(),
        name: None,
        workspace_id: "workspace-1".into(),
        branch: "styra/tidy-up-s-1".into(),
        worktree: PathBuf::from("/state/worktrees/tidy-up-s-1"),
        outcome: WorktreeCleanup::Uncommitted,
    }]);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "worktrees_cleaned");
    assert_eq!(json["data"][0]["branch"], "styra/tidy-up-s-1");
    assert_eq!(json["data"][0]["outcome"]["outcome"], "uncommitted");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

#[test]
fn listing_worktrees_names_a_workspace_or_none() {
    assert_eq!(
        serde_json::from_str::<Request>(r#"{"operation":"list_worktrees","data":{}}"#).unwrap(),
        Request::ListWorktrees { workspace_id: None }
    );

    let response = Response::Worktrees(vec![WorktreeSummary {
        workspace_id: "workspace-1".into(),
        worktree: Some(PathBuf::from("/state/worktrees/tidy-up-s-1")),
        branch: Some("styra/tidy-up-s-1".into()),
        exists: true,
        sessions: vec![WorktreeSession {
            id: "s-1".into(),
            name: None,
            completed: CompletionState::Completed,
            live: false,
        }],
    }]);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "worktrees");
    assert_eq!(json["data"][0]["sessions"][0]["id"], "s-1");
    assert_eq!(json["data"][0]["sessions"][0]["completed"], "completed");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

#[test]
fn audio_transcription_names_only_a_host_file() {
    let request = Request::TranscribeAudio {
        path: PathBuf::from("/tmp/note.wav"),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "transcribe_audio");
    assert_eq!(json["data"]["path"], "/tmp/note.wav");
    assert!(json["data"]["provider"].is_null());
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let response = Response::AudioTranscript("hello world".into());
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "audio_transcript");
    assert_eq!(json["data"], "hello world");
}

#[test]
fn audio_lifecycle_notifications_carry_only_the_error() {
    let started = serde_json::to_value(Request::AudioRecordingStarted).unwrap();
    assert_eq!(started["operation"], "audio_recording_started");

    let stopped = serde_json::to_value(Request::AudioRecordingStopped).unwrap();
    assert_eq!(stopped["operation"], "audio_recording_stopped");

    let failed = serde_json::to_value(Request::AudioTranscriptionError {
        error: "no input".into(),
    })
    .unwrap();
    assert_eq!(failed["operation"], "audio_transcription_error");
    assert_eq!(failed["data"]["error"], "no input");
}

#[test]
fn planning_carries_the_launch_inputs_without_naming_a_session() {
    let request = Request::PlanSession(PlanSession {
        workspace_id: "w-1".into(),
        selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
        launch: LaunchPolicy {
            network: Some(true),
            writable_workspace: None,
            templates: vec!["browser".into()],
            mounts: vec![LaunchMount {
                source: PathBuf::from("/srv/data"),
                destination: None,
                writable: true,
            }],
            ignore_workspace: false,
            ..LaunchPolicy::default()
        },
        create_worktree: false,
        checkout_from: None,
    });
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "plan_session");
    assert_eq!(json["data"]["workspace_id"], "w-1");
    assert_eq!(json["data"]["launch"]["network"], true);
    assert_eq!(json["data"]["launch"]["templates"][0], "browser");
    assert_eq!(json["data"]["launch"]["mounts"][0]["source"], "/srv/data");
    assert_eq!(json["data"]["launch"]["mounts"][0]["writable"], true);
    assert!(json["data"]["launch"]["mounts"][0]
        .get("destination")
        .is_none());
    // An overlay that asks for nothing says nothing on the wire, so a
    // Workspace's standing policy is what a bare launch runs under.
    assert!(json["data"]["launch"].get("ignore_workspace").is_none());
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let response = Response::SessionPlan(DrivaOptions {
        isolation_backend: "bwrap".into(),
        command: vec!["codex".into()],
        working_directory: PathBuf::from("/tmp/styra/workspace"),
        network: true,
        mounts: Vec::new(),
        base: vec![BaseCapability {
            name: "core".into(),
            description: "Run a program at all".into(),
            entries: vec![BaseEntry {
                path: PathBuf::from("/usr"),
                source: None,
            }],
            environment: Vec::new(),
        }],
        ..Default::default()
    });
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "session_plan");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

#[test]
fn resume_names_only_the_existing_session_and_launch_policy() {
    let request = Request::ResumeSession(ResumeSession {
        id: "styra-1".into(),
        launch: LaunchPolicy {
            network: Some(true),
            templates: vec!["rust".into()],
            ..LaunchPolicy::default()
        },
        selection: None,
    });
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "resume_session");
    assert_eq!(json["data"]["id"], "styra-1");
    assert_eq!(json["data"]["launch"]["templates"][0], "rust");
    assert!(
        json["data"].get("selection").is_none(),
        "a resume that keeps the stored selection names none"
    );
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    // Legacy/default and explicitly supplied wire fields use the same contract.
    {
        let request: Request = serde_json::from_str(
            r#"{"operation":"resume_session","data":{"id":"styra-1",
                 "selection":{"provider":"codex","model":"gpt-5.6-luna","effort":"low"}}}"#,
        )
        .unwrap();
        let Request::ResumeSession(resume) = request else {
            panic!("expected a resume request");
        };
        assert_eq!(
            resume.selection,
            Some(Selection::parse("codex:gpt-5.6-luna/low").unwrap())
        );
    }
}

/// A launch that names no policy of its own is the ordinary case — it runs
/// under whatever the Workspace stands for — so the field is optional and
/// its absence must read as an empty overlay rather than an error.
#[test]
fn a_launch_request_may_name_no_policy_of_its_own() {
    let request: Request = serde_json::from_str(
        r#"{"operation":"plan_session","data":{"workspace_id":"w-1",
                 "selection":{"provider":"codex","model":"gpt-5.6-sol","effort":"high"}}}"#,
    )
    .unwrap();
    let Request::PlanSession(plan) = request else {
        panic!("expected a plan request");
    };
    assert!(plan.launch.is_empty());
    assert_eq!(plan.launch.network, None);
}

#[test]
fn a_workspace_carries_the_policy_its_launches_start_from() {
    let launch = LaunchPolicy {
        network: Some(true),
        writable_workspace: None,
        templates: vec!["rust".into()],
        mounts: vec![LaunchMount {
            source: PathBuf::from("/srv/corpus"),
            destination: Some(PathBuf::from("/mnt/corpus")),
            writable: false,
        }],
        ignore_workspace: false,
        ..LaunchPolicy::default()
    };
    let request = Request::ChangeWorkspaceLaunch {
        workspace_id: "w-1".into(),
        change: WorkspaceLaunchChange::Replace(launch.clone()),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "change_workspace_launch");
    assert_eq!(json["data"]["change"]["change"], "replace");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let response = Response::WorkspaceLaunchUpdated(launch);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "workspace_launch_updated");
    assert_eq!(json["data"]["mounts"][0]["destination"], "/mnt/corpus");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

#[test]
fn a_workspace_git_repository_can_be_associated_or_cleared() {
    let request = Request::SetWorkspaceGitRepository {
        workspace_id: "w-1".into(),
        git_repository: Some(PathBuf::from("/srv/project/subdir")),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "set_workspace_git_repository");
    assert_eq!(json["data"]["workspace_id"], "w-1");
    assert_eq!(json["data"]["git_repository"], "/srv/project/subdir");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let cleared: Request = serde_json::from_str(
            r#"{"operation":"set_workspace_git_repository","data":{"workspace_id":"w-1","git_repository":null}}"#,
        )
        .unwrap();
    assert!(matches!(
        cleared,
        Request::SetWorkspaceGitRepository {
            git_repository: None,
            ..
        }
    ));
}

#[test]
fn templates_are_listed_per_workspace_with_their_descriptions() {
    let request = Request::ListTemplates {
        workspace_id: "w-1".into(),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "list_templates");
    assert_eq!(json["data"]["workspace_id"], "w-1");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let response = Response::Templates(vec![TemplateSummary {
        name: "rust".into(),
        description: "Rust toolchain".into(),
    }]);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "templates");
    assert_eq!(json["data"][0]["name"], "rust");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

#[test]
fn branch_session_names_an_optional_cutoff_and_provider() {
    let request = Request::BranchSession {
        id: "styra-1".into(),
        at_ms: Some(42),
        history: BranchHistory::SelectedOnly,
        checkout: Some(BranchCheckout::ShareSource),
        provider: Some(crate::agent::Provider::Claude),
        selection: Some(Selection::parse("claude:opus/high").unwrap()),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "branch_session");
    assert_eq!(json["data"]["at_ms"], 42);
    assert_eq!(json["data"]["history"], "selected_only");
    assert_eq!(json["data"]["checkout"], "share_source");
    assert_eq!(json["data"]["provider"], "claude");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    // A branch that keeps the whole history under the same provider says
    // nothing beyond the source id.
    let bare: Request =
        serde_json::from_str(r#"{"operation":"branch_session","data":{"id":"styra-1"}}"#).unwrap();
    assert_eq!(
        bare,
        Request::BranchSession {
            id: "styra-1".into(),
            at_ms: None,
            history: BranchHistory::ThroughSelected,
            checkout: None,
            provider: None,
            selection: None,
        }
    );

    let origin = SessionOrigin {
        session_id: "styra-1".into(),
        provider: crate::agent::Provider::Codex,
        at_ms: Some(42),
        history: BranchHistory::ThroughSelected,
    };
    let json = serde_json::to_value(&origin).unwrap();
    assert_eq!(json["history"], "through_selected");

    // Origins stored before branching offered a history choice remain
    // readable as the prefix behavior they represented.
    let legacy: SessionOrigin = serde_json::from_value(serde_json::json!({
        "session_id": "styra-1",
        "provider": "codex",
        "at_ms": 42
    }))
    .unwrap();
    assert_eq!(legacy.history, BranchHistory::ThroughSelected);
}

#[test]
fn session_tags_have_set_and_list_operations() {
    let request = Request::SetSessionTags(SetSessionTags {
        id: "styra-1".into(),
        tags: vec!["bug".into()],
    });
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "set_session_tags");
    assert_eq!(json["data"]["tags"], serde_json::json!(["bug"]));
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    assert_eq!(
        serde_json::to_value(Request::ListTags).unwrap()["operation"],
        "list_tags"
    );
}

#[test]
fn session_composer_has_get_and_set_operations() {
    let request = Request::SetSessionComposer {
        id: "styra-1".into(),
        composer: ComposerState {
            parts: vec!["fix it".into(), "the log".into()],
            focused: 1,
            history: vec!["earlier".into()],
        },
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "set_session_composer");
    assert_eq!(json["data"]["composer"]["focused"], 1);
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    assert_eq!(
        serde_json::to_value(Request::SessionComposer {
            id: "styra-1".into()
        })
        .unwrap()["operation"],
        "session_composer"
    );
    // Every field may be left out, so an older client's empty box parses.
    assert_eq!(
        serde_json::from_str::<ComposerState>("{}").unwrap(),
        ComposerState::default()
    );
}

/// The model catalog crosses the socket so a client outside this crate
/// builds a picker from it rather than from its own copy — which means
/// the effort ladder has to survive the trip beside the model it belongs
/// to.
#[test]
fn the_model_catalog_carries_each_models_effort_ladder() {
    assert_eq!(
        serde_json::to_value(Request::ListModels).unwrap()["operation"],
        "list_models"
    );
    let response = Response::Models(vec![ModelSummary {
        provider: crate::agent::Provider::Claude,
        model: "claude-haiku-4-5-20251001".into(),
        efforts: Vec::new(),
        default_effort: Effort::High,
    }]);
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "models");
    assert_eq!(json["data"][0]["provider"], "claude");
    // A model that takes no effort at all still says which value goes in
    // its Selection, and says it as an empty ladder rather than a short
    // one.
    assert_eq!(json["data"][0]["efforts"], serde_json::json!([]));
    assert_eq!(json["data"][0]["default_effort"], "high");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

/// The contract rides on the message, not the request, so `send_message`
/// and `queue_message` carry it identically.
#[test]
fn a_turn_names_its_contract_on_the_message() {
    let request = Request::SendMessage {
        id: "styra-1".into(),
        message: SendMessage {
            text: "which files handle auth?".into(),
            selection: None,
            contract: Some(Contract::Files),
        },
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "send_message");
    assert_eq!(json["data"]["message"]["contract"], "files");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);
    // Legacy/default and explicitly supplied wire fields use the same contract.
    {
        let json = serde_json::json!({
            "operation": "send_message",
            "data": { "id": "styra-1", "message": { "text": "hello" } }
        });
        let Request::SendMessage { message, .. } = serde_json::from_value::<Request>(json).unwrap()
        else {
            panic!("the operation must decode as send_message");
        };
        assert_eq!(message.contract, None);
        // And a typed turn does not appear in the JSON of an untyped one.
        let round_tripped = serde_json::to_value(&message).unwrap();
        assert!(round_tripped.get("contract").is_none());
    }
}

#[test]
fn asking_for_an_answer_may_name_a_contract_to_reread_it_under() {
    let request = Request::TurnAnswer {
        id: "styra-1".into(),
        contract: Some(Contract::Lines),
    };
    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["operation"], "turn_answer");
    assert_eq!(json["data"]["contract"], "lines");
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), request);

    let recorded = Request::TurnAnswer {
        id: "styra-1".into(),
        contract: None,
    };
    let json = serde_json::to_value(&recorded).unwrap();
    assert!(json["data"].get("contract").is_none());
    assert_eq!(serde_json::from_value::<Request>(json).unwrap(), recorded);
}

/// The value is tagged by the contract that produced it, so a client
/// dispatches on the answer alone without tracking what it asked for.
#[test]
fn an_answer_is_tagged_by_its_contract() {
    let response = Response::Answer(Answer {
        contract: Contract::Files,
        value: Some(AnswerValue::Files(vec![FileLocation {
            path: PathBuf::from("src/auth.rs"),
            line: Some(12),
            column: None,
            end_line: None,
            description: "checks the token".into(),
        }])),
        error: None,
        source: "…".into(),
    });
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "answer");
    assert_eq!(json["data"]["value"]["contract"], "files");
    assert_eq!(json["data"]["value"]["value"][0]["path"], "src/auth.rs");
    assert_eq!(json["data"]["value"]["value"][0]["line"], 12);
    // A location with no column carries none, rather than a zero that
    // would name a position the agent never gave.
    assert!(json["data"]["value"]["value"][0].get("column").is_none());
    assert!(json["data"].get("error").is_none());
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

/// A reply that missed its contract crosses the wire as an answer with no
/// value, not as a protocol error — the source is the point of sending it.
#[test]
fn an_unsatisfied_contract_is_still_an_answer() {
    let response = Response::Answer(Answer {
        contract: Contract::Json,
        value: None,
        error: Some("the answer block is not valid JSON".into()),
        source: "I had trouble with that.".into(),
    });
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["type"], "answer");
    assert_eq!(json["data"]["contract"], "json");
    assert!(json["data"].get("value").is_none());
    assert_eq!(json["data"]["source"], "I had trouble with that.");
    assert_eq!(serde_json::from_value::<Response>(json).unwrap(), response);
}

/// A queued message carries the shape it was composed with, so waiting for
/// the agent does not quietly turn a typed turn into an untyped one.
#[test]
fn a_queued_message_carries_its_contract() {
    let message = QueuedMessage::new("which files?").asking_for(Some(Contract::Files));
    let json = serde_json::to_value(&message).unwrap();
    assert_eq!(json["text"], "which files?");
    assert_eq!(json["contract"], "files");
    assert_eq!(
        serde_json::from_value::<QueuedMessage>(json).unwrap(),
        message
    );

    // An untyped one says nothing about a contract at all.
    let plain = QueuedMessage::new("hello");
    let json = serde_json::to_value(&plain).unwrap();
    assert!(json.get("contract").is_none());
    assert_eq!(
        serde_json::from_value::<QueuedMessage>(json).unwrap(),
        plain
    );
}

/// Queues written before contracts existed are arrays of bare strings.
/// Reading them as untyped messages is what keeps a resume from stranding
/// the operator's unsent work.
#[test]
fn a_queue_stored_as_bare_strings_still_loads() {
    let stored = serde_json::json!(["first", "second"]);
    let queue: Vec<QueuedMessage> = serde_json::from_value(stored).unwrap();
    assert_eq!(
        queue,
        vec![QueuedMessage::new("first"), QueuedMessage::new("second")]
    );
}

#[test]
fn every_contract_has_a_stable_wire_spelling() {
    for (contract, spelling) in [
        (Contract::Text, "text"),
        (Contract::Lines, "lines"),
        (Contract::Files, "files"),
        (Contract::Json, "json"),
    ] {
        assert_eq!(serde_json::to_value(contract).unwrap(), spelling);
        assert_eq!(contract.as_str(), spelling);
    }
}
