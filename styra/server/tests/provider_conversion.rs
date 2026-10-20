//! Provider changes retain the Session and its history; explicit branches
//! remain separate conversations. Runs with a scratch HOME for native transcripts.

use styra_server::agent::{Provider, Selection};
use styra_server::ensure_server;
use styra_server::event::Protocol;
use styra_server::journal::{self, Journal};
use styra_server::protocol::{BranchHistory, CompletionState};

#[path = "support/provider_conversion.rs"]
mod fixture;
use fixture::{scratch, stored_claude_session};

#[test]
fn switching_provider_retains_the_session_and_records_the_boundary() {
    let root = scratch();
    let home = root.join("home");
    let host = root.join("project");
    std::fs::create_dir_all(&host).unwrap();
    let store = styra_server::paths::default_store().unwrap();

    let workspace = styra_server::workspace::create(&store, &host, Some("work".into())).unwrap();
    let (source_id, source_path) = stored_claude_session(&store, &workspace.id, &home);

    // Nothing is finished with yet: the seal below is the conversion's doing,
    // not a state the Session was already in.
    assert_eq!(
        journal::read_session_completed(&source_path).unwrap(),
        CompletionState::Active
    );

    let client = ensure_server(root.join("styra.sock")).expect("the daemon should start");
    let converted = client
        .convert_session_provider(&source_id)
        .expect("converting a stored Claude Session to Codex");

    assert_eq!(converted.selection.provider, Provider::Codex);
    assert_eq!(converted.id, source_id);
    assert_eq!(converted.name.as_deref(), Some("review"));
    assert_eq!(converted.workspace_id, workspace.id);
    assert!(converted.origin.is_none());
    assert_eq!(
        journal::read_session_completed(&source_path).unwrap(),
        CompletionState::Active
    );
    assert_eq!(client.list_sessions(&workspace.id).unwrap().len(), 1);
    let replay = journal::replay(&source_path, Protocol::CodexAppServer).unwrap();
    assert!(replay.iter().any(|event| matches!(event,
        styra_server::event::AgentEvent::ProviderChanged { from, to, .. }
            if from == "claude" && to == "codex")));
    assert!(!replay
        .iter()
        .any(|event| matches!(event, styra_server::event::AgentEvent::Branched { .. })));
    assert!(
        replay.iter().any(|event| matches!(event,
        styra_server::event::AgentEvent::AgentMessage { text } if text == "the lockfile is stale"))
    );
    Journal::open(&source_path)
        .unwrap()
        .record_agent_line(
            r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m2","text":"codex answer"}}}"#,
        )
        .unwrap();
    let converted_back = client.convert_session_provider(&source_id).unwrap();
    assert_eq!(converted_back.id, source_id);
    assert_eq!(converted_back.selection.provider, Provider::Claude);
    let replay = journal::replay(&source_path, Protocol::ClaudeJsonl).unwrap();
    assert_eq!(
        replay
            .iter()
            .filter(|event| matches!(
                event,
                styra_server::event::AgentEvent::ProviderChanged { .. }
            ))
            .count(),
        2
    );
    assert_eq!(client.list_sessions(&workspace.id).unwrap().len(), 1);

    assert!(
        replay.iter().any(|event| matches!(event,
        styra_server::event::AgentEvent::AgentMessage { text } if text == "the lockfile is stale"))
    );
    assert!(replay.iter().any(|event| matches!(event,
        styra_server::event::AgentEvent::AgentMessage { text } if text == "codex answer")));

    // Explicit cross-provider branches still create separate conversations.
    let (source_id, source_path) = stored_claude_session(&store, &workspace.id, &home);
    let chosen = Selection::parse("codex:gpt-5.6-sol/high").unwrap();
    let error = client
        .branch_session_with_selection(
            &source_id,
            None,
            BranchHistory::ThroughSelected,
            Some(Provider::Claude),
            Some(chosen.clone()),
        )
        .expect_err("conflicting providers must fail before changing the source");
    assert!(error.to_string().contains("target provider"), "{error:#}");
    assert_eq!(
        journal::read_session_completed(&source_path).unwrap(),
        CompletionState::Active
    );

    let converted = client
        .branch_session_with_selection(
            &source_id,
            None,
            BranchHistory::ThroughSelected,
            Some(Provider::Codex),
            Some(chosen.clone()),
        )
        .expect("converting with the chosen model and effort");
    assert_eq!(converted.selection, chosen);
    assert_eq!(
        client
            .list_sessions(&workspace.id)
            .unwrap()
            .into_iter()
            .find(|session| session.id == converted.id)
            .unwrap()
            .selection,
        chosen
    );
    assert_eq!(
        journal::read_session_completed(&source_path).unwrap(),
        CompletionState::Sealed
    );

    // The daemon this test spawned is detached, so it would outlive the run
    // and keep its scratch store open unless it is told to stop.
    client.shutdown().ok();
    std::fs::remove_dir_all(&root).ok();
}
