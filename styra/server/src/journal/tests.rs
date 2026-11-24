use super::*;

#[test]
fn latest_user_message_time_ignores_agent_output_and_survives_reopening() {
    let dir = temp_dir("last-user-message");
    let mut journal = Journal::create(&dir).unwrap();
    assert_eq!(last_user_message_at_ms(&dir).unwrap(), 0);
    journal
        .write(&Record::User {
            at_ms: 100,
            text: "first".into(),
        })
        .unwrap();
    journal
        .write(&Record::User {
            at_ms: 200,
            text: "second".into(),
        })
        .unwrap();
    journal
        .write(&Record::Agent {
            at_ms: 900,
            raw: "agent output".into(),
            protocol: None,
        })
        .unwrap();
    drop(journal);
    let _reopened = Journal::open(&dir).unwrap();
    assert_eq!(last_user_message_at_ms(&dir).unwrap(), 200);
}

#[test]
fn a_journal_ends_mid_turn_until_the_turn_it_started_completes() {
    let dir = temp_dir("mid-turn");
    std::fs::write(
        dir.join(SESSION_META_FILE),
        r#"{"workspace_id":"w-1","protocol":"claude-jsonl","command":"claude",
                "selection":{"provider":"claude","model":"sonnet","effort":"medium"}}"#,
    )
    .unwrap();
    let mut journal = Journal::create(&dir).unwrap();
    assert!(!ends_mid_turn(&dir).unwrap(), "nothing has been sent");
    journal.record_user_message("first").unwrap();
    assert!(ends_mid_turn(&dir).unwrap(), "the message started a turn");
    journal
        .record_agent_line(r#"{"type":"assistant","message":{"content":[]}}"#)
        .unwrap();
    assert!(ends_mid_turn(&dir).unwrap(), "output is not an ending");
    journal
        .record_agent_line(r#"{"type":"result","subtype":"success","usage":{}}"#)
        .unwrap();
    assert!(!ends_mid_turn(&dir).unwrap(), "the turn completed");
    journal.record_user_message("second").unwrap();
    assert!(ends_mid_turn(&dir).unwrap(), "the next turn has no ending");
}

/// Keeps existing path-based fixtures convenient while owning their cleanup.
struct TestDirectory {
    path: PathBuf,
    _directory: tempfile::TempDir,
}

impl std::ops::Deref for TestDirectory {
    type Target = PathBuf;

    fn deref(&self) -> &PathBuf {
        &self.path
    }
}

impl AsRef<Path> for TestDirectory {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

fn temp_dir(tag: &str) -> TestDirectory {
    let directory = tempfile::Builder::new().prefix(tag).tempdir().unwrap();
    TestDirectory {
        path: directory.path().to_path_buf(),
        _directory: directory,
    }
}

#[test]
fn records_replay_in_receive_order_with_interleaved_turns() {
    let dir = temp_dir("order");
    {
        let mut journal = Journal::create(&dir).unwrap();
        journal.record_user_message("do the thing").unwrap();
        journal
            .record_agent_line(
                r#"{"type":"item.completed","item":{"type":"agent_message","text":"done"}}"#,
            )
            .unwrap();
        journal.record_user_message("thanks").unwrap();
    }

    let events = replay(&dir, Protocol::CodexJsonl).unwrap();
    assert_eq!(
        events,
        vec![
            AgentEvent::UserMessage {
                text: "do the thing".into()
            },
            AgentEvent::AgentMessage {
                text: "done".into()
            },
            AgentEvent::UserMessage {
                text: "thanks".into()
            },
        ]
    );
}

#[test]
fn a_model_change_replays_in_place_among_the_turns() {
    let dir = temp_dir("model-change");
    {
        let mut journal = Journal::create(&dir).unwrap();
        journal.record_user_message("start on sonnet").unwrap();
        // Model alone moved: the effort stays out of the record, so the
        // replayed line can say it did not change.
        journal
            .record_model_change(Some("claude-opus-5"), None)
            .unwrap();
        journal.record_user_message("now the hard part").unwrap();
    }

    let events = replay(&dir, Protocol::ClaudeJsonl).unwrap();
    assert_eq!(
        events,
        vec![
            AgentEvent::UserMessage {
                text: "start on sonnet".into()
            },
            AgentEvent::ModelChanged {
                model: Some("claude-opus-5".into()),
                effort: None,
            },
            AgentEvent::UserMessage {
                text: "now the hard part".into()
            },
        ]
    );

    // It is a host-side record, not a wire line, so the raw view of the
    // session shows only what actually crossed the pipe.
    let raw = replay_raw(&dir).unwrap();
    assert_eq!(raw.len(), 2);
}

#[test]
fn agent_lines_are_stored_verbatim() {
    let dir = temp_dir("verbatim");
    let raw = r#"{"type":"turn.completed","usage":{"input_tokens":5}}"#;
    {
        let mut journal = Journal::create(&dir).unwrap();
        journal.record_agent_line(raw).unwrap();
    }
    // Verbatim means the raw line round-trips: replaying it decodes to
    // exactly what decoding the original line directly produces.
    let events = replay(&dir, Protocol::CodexJsonl).unwrap();
    assert_eq!(events, vec![decode_line(Protocol::CodexJsonl, raw)]);
}

/// Where the agent was working is only ever said on the wire, so the last
/// line that says it is the whole answer — and a Session that moved twice
/// is in the second place, not the first.
#[test]
fn the_last_directory_the_agent_reported_is_the_one_read_back() {
    let dir = temp_dir("reported-cwd");
    {
        let mut journal = Journal::create(&dir).unwrap();
        journal
            .record_agent_line(r#"{"type":"system","subtype":"init","cwd":"/workspace"}"#)
            .unwrap();
        journal.record_user_message("make a worktree").unwrap();
        journal
            .record_agent_line(
                r#"{"type":"system","subtype":"init","cwd":"/workspace/.worktrees/audio"}"#,
            )
            .unwrap();
        // A later line that names no directory leaves the answer alone.
        journal
            .record_agent_line(r#"{"type":"assistant","message":{"role":"assistant"}}"#)
            .unwrap();
    }

    assert_eq!(
        last_reported_cwd(&dir, Protocol::ClaudeJsonl).unwrap(),
        Some("/workspace/.worktrees/audio".to_owned())
    );
}

/// A Claude Code that moves mid-turn states the new directory only in the
/// ingest context of the tool calls that follow — its `init` line for that
/// turn was written before the move and still names the old one. Reading
/// only `init` lines leaves a Session reopened here standing a whole turn
/// behind the agent, and the branch shown beside the directory with it.
#[test]
fn a_directory_entered_mid_turn_is_read_back_without_waiting_for_the_next_turn() {
    let dir = temp_dir("reported-cwd-mid-turn");
    {
        let mut journal = Journal::create(&dir).unwrap();
        journal
            .record_agent_line(r#"{"type":"system","subtype":"init","cwd":"/workspace"}"#)
            .unwrap();
        journal.record_user_message("enter the worktree").unwrap();
        journal
                .record_agent_line(
                    r#"{"type":"assistant","message":{"role":"assistant"},"wire_ingest_context":{"toolu_1":{"cwd":"/workspace/.worktrees/audio"}}}"#,
                )
                .unwrap();
    }

    assert_eq!(
        last_reported_cwd(&dir, Protocol::ClaudeJsonl).unwrap(),
        Some("/workspace/.worktrees/audio".to_owned())
    );
}

#[test]
fn restored_replay_matches_separate_views_with_malformed_and_converted_history() {
    let source = temp_dir("restored-source");
    {
        let mut journal = Journal::create(&source).unwrap();
        journal
            .record_agent_line(
                r#"{"type":"session_meta","payload":{"cwd":"/workspace/crates/ui"}}"#,
            )
            .unwrap();
    }
    let branch = temp_dir("restored-branch");
    {
        let mut journal = Journal::create(&branch).unwrap();
        journal
            .copy_branch_from(
                &source,
                Protocol::CodexJsonl,
                None,
                crate::protocol::BranchHistory::ThroughSelected,
            )
            .unwrap();
        journal.record_user_message("continue").unwrap();
        journal
            .record_model_change(Some("new-model"), None)
            .unwrap();
    }
    OpenOptions::new()
        .append(true)
        .open(branch.join(JOURNAL_FILE))
        .unwrap()
        .write_all(b"broken journal record\n")
        .unwrap();

    let restored = replay_restored(&branch, Protocol::ClaudeJsonl).unwrap();
    // Copied Codex history must supply the cwd even in a Claude session.
    assert_eq!(
        restored.last_reported_cwd.as_deref(),
        Some("/workspace/crates/ui")
    );
    assert_eq!(
        restored.events,
        replay(&branch, Protocol::ClaudeJsonl).unwrap()
    );
    assert_eq!(restored.raw, replay_raw(&branch).unwrap());
    assert_eq!(
        restored.last_reported_cwd,
        last_reported_cwd(&branch, Protocol::ClaudeJsonl).unwrap()
    );
}

#[test]
fn a_branch_replays_seed_history_with_its_original_protocol() {
    let source = temp_dir("branch-source");
    let destination = temp_dir("branch-destination");
    {
        let mut journal = Journal::create(&source).unwrap();
        journal.record_user_message("old question").unwrap();
        journal
                .record_agent_line(
                    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old answer"}]}}"#,
                )
                .unwrap();
    }
    {
        let mut journal = Journal::create(&destination).unwrap();
        journal
            .copy_branch_from(
                &source,
                Protocol::ClaudeJsonl,
                None,
                crate::protocol::BranchHistory::ThroughSelected,
            )
            .unwrap();
        journal
                .record_agent_line(
                    r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"m","text":"new answer"}}}"#,
                )
                .unwrap();
    }

    assert_eq!(
        replay(&destination, Protocol::CodexAppServer).unwrap(),
        vec![
            AgentEvent::UserMessage {
                text: "old question".into()
            },
            AgentEvent::AgentMessage {
                text: "old answer".into()
            },
            AgentEvent::AgentMessage {
                text: "new answer".into()
            },
        ]
    );
}

#[test]
fn branches_copy_the_requested_history_at_the_cutoff() {
    for (case, records, cutoff, history, expected) in [
        (
            "prefix",
            concat!(
                "{\"source\":\"user\",\"at_ms\":1,\"text\":\"first\"}\n",
                "{\"source\":\"user\",\"at_ms\":2,\"text\":\"second\"}\n",
                "{\"source\":\"user\",\"at_ms\":3,\"text\":\"third\"}\n"
            ),
            2,
            crate::protocol::BranchHistory::ThroughSelected,
            &["first", "second"][..],
        ),
        (
            "selected record",
            concat!(
                "{\"source\":\"user\",\"at_ms\":10,\"text\":\"first\"}\n",
                "{\"source\":\"user\",\"at_ms\":20,\"text\":\"selected\"}\n",
                "{\"source\":\"user\",\"at_ms\":30,\"text\":\"later\"}\n"
            ),
            21,
            crate::protocol::BranchHistory::SelectedOnly,
            &["selected"][..],
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        std::fs::create_dir(&source).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(source.join(JOURNAL_FILE), records).unwrap();
        {
            let mut journal = Journal::create(&destination).unwrap();
            journal
                .copy_branch_from(&source, Protocol::ClaudeJsonl, Some(cutoff), history)
                .unwrap();
        }
        // Compare the full event sequence: unexpected non-message events
        // must not be hidden by filtering the replay.
        let expected: Vec<_> = expected
            .iter()
            .map(|text| AgentEvent::UserMessage {
                text: (*text).into(),
            })
            .collect();
        assert_eq!(
            replay(&destination, Protocol::CodexAppServer).unwrap(),
            expected,
            "{case}"
        );
    }
}

/// A branched Session's journal is not in time order: its `branched from`
/// markers are dated when the branch was made, and the history copied
/// between them is older. Branching it again at an entry in that inherited
/// history still has to copy that entry — the marker at the top is newer
/// than the cutoff, but it is not where the history stops.
#[test]
fn branching_a_branch_inside_its_inherited_history_copies_the_selected_entry() {
    let source = temp_dir("branch-of-branch-source");
    std::fs::write(
            source.join(JOURNAL_FILE),
            concat!(
                "{\"source\":\"branch\",\"at_ms\":100,\"direction\":\"from\",\"session\":\"styra-origin\"}\n",
                "{\"source\":\"user\",\"at_ms\":10,\"text\":\"inherited first\"}\n",
                "{\"source\":\"user\",\"at_ms\":20,\"text\":\"inherited second\"}\n",
                "{\"source\":\"branch\",\"at_ms\":100,\"direction\":\"from\",\"session\":\"styra-origin\"}\n",
                "{\"source\":\"user\",\"at_ms\":110,\"text\":\"own\"}\n"
            ),
        )
        .unwrap();

    for history in [
        crate::protocol::BranchHistory::ThroughSelected,
        crate::protocol::BranchHistory::SelectedOnly,
    ] {
        let destination = temp_dir("branch-of-branch-destination");
        {
            let mut journal = Journal::create(&destination).unwrap();
            journal
                .copy_branch_from(&source, Protocol::ClaudeJsonl, Some(10), history)
                .unwrap();
        }
        let messages: Vec<_> = replay(&destination, Protocol::ClaudeJsonl)
            .unwrap()
            .into_iter()
            .filter_map(|event| match event {
                AgentEvent::UserMessage { text } => Some(text),
                _ => None,
            })
            .collect();

        assert_eq!(messages, vec!["inherited first"], "{history:?}");
    }
}

/// A branch point resolves to an operator message only when their message
/// is what the cutoff lands on; landing on an agent line resolves to
/// nothing, since the provider's own copy of that line needs no help being
/// found by timestamp.
#[test]
fn a_branch_point_names_the_operator_message_it_lands_on() {
    let directory = temp_dir("branch-point-operator-message");
    std::fs::write(
            directory.join(JOURNAL_FILE),
            concat!(
                "{\"source\":\"user\",\"at_ms\":10,\"text\":\"first\"}\n",
                "{\"source\":\"agent\",\"at_ms\":20,\"raw\":\"{}\"}\n",
                "{\"source\":\"user\",\"at_ms\":30,\"text\":\"try again\"}\n",
                "{\"source\":\"branch\",\"at_ms\":31,\"direction\":\"to\",\"session\":\"styra-other\"}\n",
                "{\"source\":\"agent\",\"at_ms\":40,\"raw\":\"{}\"}\n"
            ),
        )
        .unwrap();

    assert_eq!(
        operator_message_at(&directory, 10).unwrap().as_deref(),
        Some("first")
    );
    assert_eq!(operator_message_at(&directory, 20).unwrap(), None);
    // Past the operator's message and its branch marker, but before the
    // reply: still their message.
    assert_eq!(
        operator_message_at(&directory, 35).unwrap().as_deref(),
        Some("try again")
    );
    assert_eq!(operator_message_at(&directory, 40).unwrap(), None);
    assert_eq!(operator_message_at(&directory, 5).unwrap(), None);
}

#[test]
fn a_branch_marker_replays_as_a_link_to_the_other_session() {
    let directory = temp_dir("branch-marker");
    {
        let mut journal = Journal::create(&directory).unwrap();
        journal
            .record_branch(BranchDirection::From, "styra-source")
            .unwrap();
        journal.record_user_message("carry on here").unwrap();
    }

    assert_eq!(
        replay(&directory, Protocol::ClaudeJsonl).unwrap(),
        vec![
            AgentEvent::Branched {
                direction: BranchDirection::From,
                session: "styra-source".into(),
                name: None,
            },
            AgentEvent::UserMessage {
                text: "carry on here".into()
            },
        ]
    );
    // A marker is not a wire line, so it stays out of the raw view; only
    // the operator's message is there.
    let raw = replay_raw(&directory).unwrap();
    assert_eq!(raw.len(), 1);
    assert_eq!(raw[0].text, "carry on here");
}

/// Branching only the selected entry must seed the new Session with a real
/// entry: a marker is a boundary the operator cannot have selected, so one
/// sitting at the cutoff is skipped in favour of the message before it.
#[test]
fn a_selected_only_branch_never_resolves_to_a_branch_marker() {
    let source = temp_dir("branch-marker-source");
    let destination = temp_dir("branch-marker-destination");
    std::fs::write(
            source.join(JOURNAL_FILE),
            concat!(
                "{\"source\":\"user\",\"at_ms\":10,\"text\":\"selected\"}\n",
                "{\"source\":\"branch\",\"at_ms\":20,\"direction\":\"to\",\"session\":\"styra-other\"}\n",
                "{\"source\":\"user\",\"at_ms\":30,\"text\":\"later\"}\n"
            ),
        )
        .unwrap();
    {
        let mut journal = Journal::create(&destination).unwrap();
        journal
            .copy_branch_from(
                &source,
                Protocol::ClaudeJsonl,
                Some(21),
                crate::protocol::BranchHistory::SelectedOnly,
            )
            .unwrap();
    }

    assert_eq!(
        replay(&destination, Protocol::ClaudeJsonl).unwrap(),
        vec![AgentEvent::UserMessage {
            text: "selected".into()
        }]
    );
}

fn test_profile(name: &str, protocol: Protocol) -> Profile {
    Profile {
        name: name.into(),
        command: vec!["true".into()],
        protocol,
        mounts: Vec::new(),
        environment: Default::default(),
        network: false,
        message_format: crate::agent::MessageFormat::PlainLine,
        single_turn: false,
    }
}

struct SessionFixture {
    journal: Journal,
    host: PathBuf,
    root: PathBuf,
    workspace: crate::protocol::WorkspaceSummary,
    id: String,
    _directory: tempfile::TempDir,
}

fn workspace_session(
    workspace_name: Option<String>,
    session_name: Option<String>,
) -> SessionFixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("store");
    let host = directory.path().join("host");
    std::fs::create_dir(&host).unwrap();
    let workspace = crate::workspace::create(&root, &host, workspace_name).unwrap();
    let profile = test_profile("codex", Protocol::CodexJsonl);
    let selection = crate::agent::Selection::new(crate::agent::Provider::Codex);
    let (journal, id) =
        Journal::create_in_workspace(&root, &workspace.id, &profile, &selection, session_name)
            .unwrap();
    SessionFixture {
        host,
        journal,
        root,
        workspace,
        id,
        _directory: directory,
    }
}

#[test]
fn workspace_sessions_are_nested_and_record_their_owner() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap();
    assert_eq!(
        directory,
        crate::workspace::sessions_dir(&fixture.root, &fixture.workspace.id).join(&fixture.id)
    );
    assert_eq!(
        read_session_workspace_id(directory).unwrap(),
        fixture.workspace.id
    );
    assert_eq!(read_provider_session_id(directory).unwrap(), None);
    store_provider_session_id(directory, "provider-1").unwrap();
    assert_eq!(
        read_provider_session_id(directory).unwrap().as_deref(),
        Some("provider-1")
    );

    let sessions = list_workspace_sessions(&fixture.root, &fixture.workspace.id).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, fixture.id);
    assert_eq!(sessions[0].workspace_id, fixture.workspace.id);
    assert_eq!(
        sessions[0].selection,
        crate::agent::Selection::new(crate::agent::Provider::Codex)
    );
    assert_eq!(
        crate::workspace::get(&fixture.root, &fixture.workspace.id)
            .unwrap()
            .session_count,
        1
    );
}

/// A turn runs from the snapshot before its message — or, where that was
/// skipped, the one the turn before ended on — to its own end. A turn
/// whose start was not snapshotted has no pair.
#[test]
fn a_turn_is_bracketed_by_the_snapshots_around_it() {
    let directory = temp_dir("snapshot-turns");
    let take = |moment, turn| {
        append_snapshot(
            &directory,
            SnapshotState {
                moment,
                turn: Some(turn),
                checkout: directory.clone(),
                branch: None,
                commit: Some(format!("{moment:?}-{turn}")),
                diff: None,
            },
        )
        .unwrap()
        .sequence
    };
    use SnapshotMoment::{BeforeMessage, TurnEnded};
    // Snapshots turned on in the middle of turn 1.
    take(TurnEnded, 1);
    take(TurnEnded, 2);
    take(BeforeMessage, 3);
    take(TurnEnded, 3);

    let pair = |turn| match turn_snapshots(&directory, turn).unwrap() {
        Some(TurnSnapshots::Ended(start, end)) => Some((start.sequence, end.sequence)),
        Some(TurnSnapshots::Running(_)) => panic!("turn {turn} has ended"),
        None => None,
    };
    let running = |turn| match turn_snapshots(&directory, turn).unwrap() {
        Some(TurnSnapshots::Running(from)) => Some(from.sequence),
        _ => None,
    };
    assert_eq!(pair(1), None, "nothing before the first");
    assert_eq!(pair(2), Some((1, 2)), "its message's snapshot was skipped");
    assert_eq!(pair(3), Some((3, 4)));
    assert_eq!(
        running(4),
        Some(4),
        "not ended: from the previous turn's end"
    );
    take(BeforeMessage, 4);
    assert_eq!(running(4), Some(5), "from its own message's snapshot");
    take(BeforeMessage, 5);
    assert_eq!(
        turn_snapshots(&directory, 4).unwrap(),
        None,
        "a later turn has started, so turn 4's end was never recorded"
    );

    // Woken by its own background task, the agent completes again without
    // an operator message: the turn runs from its message to the last end.
    take(TurnEnded, 5);
    take(TurnEnded, 5);
    assert_eq!(pair(5), Some((6, 8)));

    // Turns 6 and 7 were never recorded: turn 5's end is not turn 8's
    // start, and comparing with it would hand turn 8 their changes.
    take(TurnEnded, 8);
    assert_eq!(pair(8), None);
}

/// Each snapshot is numbered in turn, with a diff file only for a
/// checkout that had changes.
#[test]
fn snapshots_are_stored_with_the_session_and_appended_in_order() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap();

    assert!(read_snapshots(directory).unwrap().is_empty());

    let clean = append_snapshot(
        directory,
        SnapshotState {
            moment: SnapshotMoment::BeforeMessage,
            turn: Some(1),
            checkout: fixture.host.clone(),
            branch: Some("main".into()),
            commit: Some("abc".into()),
            diff: None,
        },
    )
    .unwrap();
    let dirty = append_snapshot(
        directory,
        SnapshotState {
            moment: SnapshotMoment::BeforeMessage,
            turn: Some(1),
            checkout: fixture.host.clone(),
            branch: None,
            commit: Some("abc".into()),
            diff: Some(b"diff --git a/x b/x\n".to_vec()),
        },
    )
    .unwrap();

    assert_eq!((clean.sequence, clean.diff.as_ref()), (1, None));
    assert_eq!(dirty.sequence, 2);
    let diff = directory.join(dirty.diff.as_ref().unwrap());
    assert_eq!(std::fs::read(diff).unwrap(), b"diff --git a/x b/x\n");
    assert_eq!(read_snapshots(directory).unwrap(), vec![clean, dirty]);

    // The last snapshot is compared to the byte, diff and all.
    let same = |diff: &[u8]| SnapshotState {
        moment: SnapshotMoment::BeforeMessage,
        turn: Some(1),
        checkout: fixture.host.clone(),
        branch: None,
        commit: Some("abc".into()),
        diff: Some(diff.to_vec()),
    };
    assert!(unchanged_since_last_snapshot(directory, &same(b"diff --git a/x b/x\n")).unwrap());
    assert!(!unchanged_since_last_snapshot(directory, &same(b"diff --git a/y b/y\n")).unwrap());
    let mut clean_again = same(b"");
    clean_again.diff = None;
    assert!(!unchanged_since_last_snapshot(directory, &clean_again).unwrap());
}

/// The contract outlives the interaction that used it: an answer can be
/// parsed from a stored session long after the agent process is gone, and
/// a later turn's contract replaces an earlier one.
#[test]
fn the_last_typed_turns_contract_is_stored_with_the_session() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap();

    assert_eq!(read_session_contract(directory).unwrap(), None);
    store_session_contract(directory, Contract::Files).unwrap();
    assert_eq!(
        read_session_contract(directory).unwrap(),
        Some(Contract::Files)
    );
    // A session mixes typed turns freely; the answer belongs to the last
    // question asked, not the first.
    store_session_contract(directory, Contract::Json).unwrap();
    assert_eq!(
        read_session_contract(directory).unwrap(),
        Some(Contract::Json)
    );
    // Recording a contract must not disturb the rest of the metadata.
    assert_eq!(
        read_session_workspace_id(directory).unwrap(),
        fixture.workspace.id
    );
    assert_eq!(
        read_session_meta(directory).unwrap().protocol,
        Protocol::CodexJsonl
    );
}

/// Until the operator answers, a Session's turns are committed only in a
/// checkout of its own; their answer, once given, is remembered by the
/// Session rather than by the interaction that happened to be open.
#[test]
fn committing_each_turn_defaults_to_the_checkout_and_is_stored_with_the_session() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap();

    let stored = read_session_auto_commit(directory).unwrap();
    assert_eq!(stored, None);
    assert!(resolve_auto_commit(stored, true), "on in a linked checkout");
    assert!(!resolve_auto_commit(stored, false), "off in the Workspace");

    store_session_auto_commit(directory, false).unwrap();
    assert!(!resolve_auto_commit(
        read_session_auto_commit(directory).unwrap(),
        true
    ));
    store_session_auto_commit(directory, true).unwrap();
    assert!(resolve_auto_commit(
        read_session_auto_commit(directory).unwrap(),
        false
    ));
    assert_eq!(
        read_session_workspace_id(directory).unwrap(),
        fixture.workspace.id
    );
}

/// The retry a rate limit triggers resumes the Session as a new
/// interaction, so the operator's standing "keep at it" has to be stored
/// with the Session — held on the interaction alone it would apply once
/// and then be gone, which is the opposite of what it says.
#[test]
fn keeping_at_it_after_a_rate_limit_is_stored_with_the_session() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap();

    // Nothing is done on the operator's behalf until they ask.
    assert!(!read_session_auto_retry(directory).unwrap());
    store_session_auto_retry(directory, true).unwrap();
    assert!(read_session_auto_retry(directory).unwrap());
    store_session_auto_retry(directory, false).unwrap();
    assert!(!read_session_auto_retry(directory).unwrap());

    // And recording it disturbs nothing else about the Session.
    store_session_auto_retry(directory, true).unwrap();
    assert_eq!(
        read_session_workspace_id(directory).unwrap(),
        fixture.workspace.id
    );
    assert_eq!(
        read_session_meta(directory).unwrap().protocol,
        Protocol::CodexJsonl
    );
}

/// Completion is a property of the Session, not of whichever interaction
/// happened to be open when the operator set it, so it has to round-trip
/// through the same stored metadata every other Session property does.
#[test]
fn completion_is_stored_with_the_session() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap();

    assert_eq!(
        read_session_completed(directory).unwrap(),
        CompletionState::Active
    );
    store_session_completed(directory, CompletionState::Completed).unwrap();
    assert_eq!(
        read_session_completed(directory).unwrap(),
        CompletionState::Completed
    );
    store_session_completed(directory, CompletionState::Active).unwrap();
    assert_eq!(
        read_session_completed(directory).unwrap(),
        CompletionState::Active
    );

    store_session_completed(directory, CompletionState::Abandoned).unwrap();
    assert_eq!(
        read_session_completed(directory).unwrap(),
        CompletionState::Abandoned
    );

    // Sealing behaves the same way at the storage layer; only the server
    // refuses to undo it.
    store_session_completed(directory, CompletionState::Sealed).unwrap();
    assert_eq!(
        read_session_completed(directory).unwrap(),
        CompletionState::Sealed
    );

    // And recording it disturbs nothing else about the Session.
    store_session_completed(directory, CompletionState::Completed).unwrap();
    assert_eq!(
        read_session_workspace_id(directory).unwrap(),
        fixture.workspace.id
    );
    assert_eq!(
        read_session_meta(directory).unwrap().protocol,
        Protocol::CodexJsonl
    );
}

/// A finished Session reads as active while anything branched from it,
/// however far down, is; a sealed one never does.
#[test]
fn completion_follows_active_branches_at_any_depth() {
    let mut lineage = Lineage::default();
    let origin = |parent: &str| SessionOrigin {
        session_id: parent.into(),
        provider: crate::agent::Provider::Codex,
        at_ms: None,
        history: Default::default(),
    };
    lineage.add("root".into(), None, CompletionState::Completed);
    lineage.add("sealed".into(), None, CompletionState::Sealed);
    lineage.add(
        "child".into(),
        Some(&origin("root")),
        CompletionState::Abandoned,
    );
    lineage.add(
        "other".into(),
        Some(&origin("sealed")),
        CompletionState::Active,
    );
    // A malformed loop must not hang the walk.
    lineage.add(
        "loop".into(),
        Some(&origin("loop")),
        CompletionState::Completed,
    );

    assert_eq!(
        lineage.completion("root", CompletionState::Completed),
        CompletionState::Completed
    );
    assert_eq!(
        lineage.completion("loop", CompletionState::Completed),
        CompletionState::Completed
    );

    lineage.add(
        "grandchild".into(),
        Some(&origin("child")),
        CompletionState::Active,
    );
    assert_eq!(
        lineage.completion("root", CompletionState::Completed),
        CompletionState::Active
    );
    assert_eq!(
        lineage.completion("child", CompletionState::Abandoned),
        CompletionState::Active
    );
    assert_eq!(
        lineage.completion("sealed", CompletionState::Sealed),
        CompletionState::Sealed
    );
}

/// Sessions stored before completion became an enum carry a JSON boolean
/// in the same field, and there are only two spellings of it on disk: a
/// completed Session wrote `true`, and every other one omitted the field.
/// Both have to keep loading, or upgrading would make exactly the
/// Sessions the operator had finished with unreadable.
#[test]
fn metadata_written_before_completion_was_an_enum_still_loads() {
    let fixture = workspace_session(Some("work".into()), None);
    let directory = fixture.journal.path().parent().unwrap().to_path_buf();
    drop(fixture.journal);
    let meta_path = directory.join(SESSION_META_FILE);

    let text = std::fs::read_to_string(&meta_path).unwrap();
    let mut stored: serde_json::Value = serde_json::from_str(&text).unwrap();
    stored["completed"] = serde_json::Value::Bool(true);
    std::fs::write(&meta_path, serde_json::to_string_pretty(&stored).unwrap()).unwrap();

    assert_eq!(
        read_session_completed(&directory).unwrap(),
        CompletionState::Completed,
        "a Session stored as completed reads back as completed"
    );
}

#[test]
fn a_queued_message_keeps_its_contract_across_a_restart() {
    let dir = temp_dir("queue-contract");
    let queued = vec![
        QueuedMessage::new("which files?").asking_for(Some(Contract::Files)),
        QueuedMessage::new("and then?"),
    ];
    write_queued_messages(&dir, &queued).unwrap();
    assert_eq!(read_queued_messages(&dir).unwrap(), queued);
}

/// A queue file written before contracts existed is an array of strings.
/// It has to load, or resuming that session throws away messages the
/// operator wrote and never got to send.
#[test]
fn a_queue_file_of_bare_strings_still_loads() {
    let dir = temp_dir("queue-legacy");
    std::fs::write(dir.join(QUEUE_FILE), r#"["first","second"]"#).unwrap();
    assert_eq!(
        read_queued_messages(&dir).unwrap(),
        vec![QueuedMessage::new("first"), QueuedMessage::new("second")]
    );
}

/// The last-activity time is read from the journal's write time; a
/// Session that has not written one yet reports none, and orders by its
/// creation instead.
#[test]
fn the_last_event_time_comes_from_the_journal_write_time() {
    let dir = temp_dir("last-event");
    assert_eq!(last_event_at_ms(&dir), None);
    std::fs::write(dir.join(JOURNAL_FILE), "{}\n").unwrap();
    let written = last_event_at_ms(&dir).expect("a written journal has a write time");
    assert!(written > 0);
}

/// Sessions written before typed turns existed have no `contract` field,
/// and must still load — as untyped, which is what they were.
#[test]
fn a_session_stored_without_a_contract_reads_as_untyped() {
    let dir = temp_dir("contract-absent");
    std::fs::write(
        dir.join(SESSION_META_FILE),
        r#"{"workspace_id":"w-1","protocol":"codex-jsonl","command":"codex",
                "selection":{"provider":"codex","model":"gpt-5-codex","effort":"medium"}}"#,
    )
    .unwrap();
    assert_eq!(read_session_contract(&dir).unwrap(), None);
}

#[test]
fn a_corrupt_record_is_surfaced_not_fatal() {
    let dir = temp_dir("corrupt");
    std::fs::write(dir.join(JOURNAL_FILE), "not a record\n").unwrap();
    let events = replay(&dir, Protocol::CodexJsonl).unwrap();
    assert!(matches!(events.as_slice(), [AgentEvent::Malformed { .. }]));
}

#[test]
fn sessions_sort_newest_first_with_unknown_age_last() {
    let summary = |created_at_ms: Option<u64>| SessionSummary {
        id: format!("{created_at_ms:?}"),
        name: None,
        first_prompt: None,
        tags: Vec::new(),
        workspace_id: "w-1".into(),
        path: PathBuf::new(),
        selection: crate::agent::Selection::new(crate::agent::Provider::Codex),
        age: String::new(),
        created_at_ms,
        last_event_at_ms: created_at_ms,
        last_event_age: String::new(),
        origin: None,
        branched_from: None,
        completed: CompletionState::Active,
        integration: None,
    };
    let mut sessions = vec![
        summary(Some(100)),
        summary(None),
        summary(Some(300)),
        summary(Some(200)),
    ];
    sort_newest_first(&mut sessions);
    let order: Vec<Option<u64>> = sessions.iter().map(|s| s.created_at_ms).collect();
    assert_eq!(order, vec![Some(300), Some(200), Some(100), None]);
}

#[test]
fn session_created_at_ms_parses_the_leading_timestamp_field() {
    assert_eq!(session_created_at_ms("0000000123456-42-7"), Some(123456));
    assert_eq!(session_created_at_ms("not-an-id"), None);
    assert_eq!(session_created_at_ms(""), None);
}

#[test]
fn names_are_normalized_and_first_messages_make_bounded_defaults() {
    assert_eq!(
        normalize_session_name(Some("  Fix the picker  ")).unwrap(),
        Some("Fix the picker".into())
    );
    assert_eq!(normalize_session_name(Some("   ")).unwrap(), None);
    assert!(normalize_session_name(Some("bad\nname")).is_err());

    assert_eq!(
        name_from_message(Some("\n  Fix   the picker\nwithout regressions ")),
        Some("Fix the picker without regressions".into())
    );
    let long = "x".repeat(100);
    let derived = name_from_message(Some(&long)).unwrap();
    assert_eq!(derived.chars().count(), 61);
    assert!(derived.ends_with('…'));
}

#[test]
fn first_prompt_reads_the_opening_operator_message_without_replaying_the_journal() {
    let directory = temp_dir("first-prompt");
    let mut journal = Journal::create(&directory).unwrap();
    journal.record_agent_line("provider prelude").unwrap();
    journal
        .record_user_message("Find the flaky checkout test")
        .unwrap();
    journal.record_user_message("Then fix it").unwrap();

    assert_eq!(
        first_prompt_at(&directory).unwrap().as_deref(),
        Some("Find the flaky checkout test")
    );
}

#[test]
fn a_session_never_branched_owns_its_first_prompt() {
    let directory = temp_dir("own-first-prompt-plain");
    let mut journal = Journal::create(&directory).unwrap();
    journal.record_user_message("Find the flaky test").unwrap();
    // A child branching off this Session is no boundary of its own.
    journal.record_branch(BranchDirection::To, "child").unwrap();
    journal.record_user_message("Then fix it").unwrap();

    assert_eq!(
        own_first_prompt_at(&directory).unwrap().as_deref(),
        Some("Find the flaky test")
    );
}

/// A branch of a branch carries both of its ancestors' markers in its
/// copied history; only the last one is its own branch point.
#[test]
fn a_branch_owns_the_first_prompt_after_its_branch_point() {
    let directory = temp_dir("own-first-prompt-branch");
    let mut journal = Journal::create(&directory).unwrap();
    journal
        .record_branch(BranchDirection::From, "parent")
        .unwrap();
    journal
        .record_branch(BranchDirection::From, "grandparent")
        .unwrap();
    journal.record_user_message("Find the flaky test").unwrap();
    journal
        .record_branch(BranchDirection::From, "grandparent")
        .unwrap();
    journal.record_user_message("Try retrying").unwrap();
    journal
        .record_branch(BranchDirection::To, "sibling")
        .unwrap();
    journal
        .record_branch(BranchDirection::From, "parent")
        .unwrap();
    journal.record_agent_line("provider prelude").unwrap();
    journal.record_user_message("Try a lock instead").unwrap();
    journal.record_user_message("And test it").unwrap();

    assert_eq!(
        own_first_prompt_at(&directory).unwrap().as_deref(),
        Some("Try a lock instead")
    );
    assert_eq!(
        first_prompt_at(&directory).unwrap().as_deref(),
        Some("Find the flaky test")
    );
}

#[test]
fn a_branch_with_no_message_of_its_own_has_no_own_prompt() {
    let directory = temp_dir("own-first-prompt-empty");
    let mut journal = Journal::create(&directory).unwrap();
    journal
        .record_branch(BranchDirection::From, "parent")
        .unwrap();
    journal.record_user_message("Find the flaky test").unwrap();
    journal
        .record_branch(BranchDirection::From, "parent")
        .unwrap();

    assert_eq!(own_first_prompt_at(&directory).unwrap(), None);
}

#[test]
fn a_switched_model_is_what_the_session_is_later_listed_and_resumed_with() {
    let fixture = workspace_session(None, None);
    let directory = fixture.journal.path().parent().unwrap();
    store_provider_session_id(directory, "provider-1").unwrap();

    let switched = crate::agent::Selection::parse("codex:gpt-5.6-terra/high").unwrap();
    store_session_selection(directory, &switched).unwrap();

    assert_eq!(read_session_meta(directory).unwrap().selection, switched);
    assert_eq!(
        list_workspace_sessions(&fixture.root, &fixture.workspace.id).unwrap()[0].selection,
        switched
    );
    // The switch is metadata like any other: it leaves the rest intact.
    assert_eq!(
        read_provider_session_id(directory).unwrap().as_deref(),
        Some("provider-1")
    );
}

#[test]
fn session_name_updates_preserve_other_metadata() {
    let root = temp_dir("session-name");
    let host = temp_dir("session-name-host");
    let workspace = crate::workspace::create(&root, &host, None).unwrap();
    let profile = test_profile("codex", Protocol::CodexJsonl);
    let selection = crate::agent::Selection::new(crate::agent::Provider::Codex);
    let (journal, _) = Journal::create_in_workspace(
        &root,
        &workspace.id,
        &profile,
        &selection,
        Some("Initial".into()),
    )
    .unwrap();
    let directory = journal.path().parent().unwrap();
    store_provider_session_id(directory, "provider-1").unwrap();
    assert_eq!(
        store_session_name(directory, Some(" Renamed ")).unwrap(),
        Some("Renamed".into())
    );
    let listed = list_workspace_sessions(&root, &workspace.id).unwrap();
    assert_eq!(listed[0].name.as_deref(), Some("Renamed"));
    assert_eq!(
        read_provider_session_id(directory).unwrap().as_deref(),
        Some("provider-1")
    );
    assert_eq!(store_session_name(directory, None).unwrap(), None);
    assert_eq!(
        list_workspace_sessions(&root, &workspace.id).unwrap()[0].name,
        None
    );
}

#[test]
fn session_tags_are_normalized_and_durable() {
    let fixture = workspace_session(None, None);
    let directory = fixture.journal.path().parent().unwrap();

    assert_eq!(
        store_session_tags(directory, &[" urgent ".into(), "bug".into(), "bug".into()]).unwrap(),
        ["bug", "urgent"]
    );
    assert_eq!(
        list_workspace_sessions(&fixture.root, &fixture.workspace.id).unwrap()[0].tags,
        ["bug", "urgent"]
    );
}

#[test]
fn the_message_box_is_durable_and_its_history_bounded() {
    let dir = temp_dir("composer");
    assert_eq!(read_composer(&dir).unwrap(), ComposerState::default());

    let history: Vec<String> = (0..COMPOSER_HISTORY_LIMIT + 5)
        .map(|index| format!("sent {index}"))
        .collect();
    let composer = ComposerState {
        parts: vec!["half".into(), "written".into()],
        focused: 1,
        history,
    };
    write_composer(&dir, &composer).unwrap();
    let stored = read_composer(&dir).unwrap();
    assert_eq!(stored.parts, composer.parts);
    assert_eq!(stored.focused, 1);
    assert_eq!(stored.history.len(), COMPOSER_HISTORY_LIMIT);
    // The oldest go; the newest stay, still oldest first.
    assert_eq!(stored.history[0], "sent 5");
    assert_eq!(
        stored.history.last().unwrap(),
        &format!("sent {}", COMPOSER_HISTORY_LIMIT + 4)
    );
}

#[test]
fn humanize_age_buckets_elapsed_time() {
    assert_eq!(humanize_age(1_000, None), "unknown");
    assert_eq!(humanize_age(10_000, Some(9_500)), "just now");
    assert_eq!(humanize_age(200_000, Some(0)), "3m ago");
    assert_eq!(humanize_age(3_600_000 * 2, Some(0)), "2h ago");
    assert_eq!(humanize_age(86_400_000 * 5, Some(0)), "5d ago");
}
