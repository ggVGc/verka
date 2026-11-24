use super::*;
use crate::protocol::BranchHistory;
use genta::session::{Message, Role, Session};

fn seed(transcripts: &MemoryTranscripts, provider: Provider, id: &str) -> Session {
    let session = Session {
        id: id.into(),
        cwd: "/original".into(),
        timestamp: "2026-08-21T10:00:00.000Z".into(),
        messages: vec![
            Message {
                role: Role::User,
                text: "first question".into(),
                timestamp: Some("2026-08-21T10:00:00.000Z".into()),
            },
            Message {
                role: Role::Assistant,
                text: "first answer".into(),
                timestamp: Some("2026-08-21T10:00:01.000Z".into()),
            },
            Message {
                role: Role::User,
                text: "second question".into(),
                timestamp: Some("2026-08-21T10:00:02.000Z".into()),
            },
        ],
    };
    let text = genta::session::serialize(&session, super::super::native_session_format(provider));
    transcripts
        .write(provider, id, Path::new(&session.cwd), &text)
        .unwrap();
    session
}

#[test]
fn native_ids_are_scoped_to_the_provider_and_missing_sessions_fail() {
    let transcripts = MemoryTranscripts::default();
    transcripts
        .write(Provider::Claude, "same-id", Path::new("/one"), "claude")
        .unwrap();
    transcripts
        .write(Provider::Codex, "same-id", Path::new("/two"), "codex")
        .unwrap();
    assert_eq!(
        transcripts.read(Provider::Claude, "same-id").unwrap(),
        "claude"
    );
    assert_eq!(
        transcripts.read(Provider::Codex, "same-id").unwrap(),
        "codex"
    );
    assert!(transcripts.read(Provider::Codex, "missing").is_err());
}

#[test]
fn branches_store_the_selected_history_in_the_new_checkout_and_preserve_the_source() {
    // Both workflows share this conversion/storage path. Unlike a switch,
    // an explicit branch can retain a prefix or just the selected message.
    for provider in [Provider::Claude, Provider::Codex] {
        for history in [BranchHistory::ThroughSelected, BranchHistory::SelectedOnly] {
            let transcripts = MemoryTranscripts::default();
            let source = seed(&transcripts, provider, "source");
            let original = transcripts.read(provider, "source").unwrap();
            write_converted(
                &transcripts,
                &original,
                provider,
                provider,
                "branch",
                Path::new("/branch-checkout"),
                super::super::BranchPoint {
                    at_ms: Some(
                        super::super::parse_rfc3339_ms("2026-08-21T10:00:01.000Z").unwrap(),
                    ),
                    history,
                    selected_operator_message: None,
                },
            )
            .unwrap();

            let branch = genta::session::parse(
                &transcripts.read(provider, "branch").unwrap(),
                super::super::native_session_format(provider),
            )
            .unwrap();
            assert_eq!(branch.id, "branch");
            assert_eq!(branch.cwd, "/branch-checkout");
            let expected = match history {
                BranchHistory::ThroughSelected => &source.messages[..2],
                BranchHistory::SelectedOnly => &source.messages[1..2],
            };
            assert_eq!(branch.messages, expected);
            assert_eq!(transcripts.read(provider, "source").unwrap(), original);
            assert_eq!(transcripts.len(), 2);
        }
    }
}

#[test]
fn full_history_can_switch_in_both_directions_using_the_same_storage() {
    for (from, to) in [
        (Provider::Claude, Provider::Codex),
        (Provider::Codex, Provider::Claude),
    ] {
        let transcripts = MemoryTranscripts::default();
        let source = seed(&transcripts, from, "source");
        let original = transcripts.read(from, "source").unwrap();
        write_converted(
            &transcripts,
            &original,
            from,
            to,
            "converted",
            Path::new("/original"),
            super::super::BranchPoint {
                at_ms: None,
                history: BranchHistory::ThroughSelected,
                selected_operator_message: None,
            },
        )
        .unwrap();
        let converted = genta::session::parse(
            &transcripts.read(to, "converted").unwrap(),
            super::super::native_session_format(to),
        )
        .unwrap();
        assert_eq!(converted.messages, source.messages);
        assert_eq!(converted.cwd, source.cwd);
        assert_eq!(converted.id, "converted");
        assert_eq!(transcripts.read(from, "source").unwrap(), original);
    }
}

#[test]
fn a_failed_write_or_invalid_transcript_keeps_existing_storage_unchanged() {
    let transcripts = MemoryTranscripts::default();
    seed(&transcripts, Provider::Claude, "source");
    let original = transcripts.read(Provider::Claude, "source").unwrap();
    for (source, fail_write) in [(original.as_str(), true), ("invalid json", false)] {
        transcripts.fail_writes(fail_write);
        assert!(write_converted(
            &transcripts,
            source,
            Provider::Claude,
            Provider::Codex,
            "destination",
            Path::new("/original"),
            super::super::BranchPoint {
                at_ms: None,
                history: BranchHistory::ThroughSelected,
                selected_operator_message: None
            },
        )
        .is_err());
        assert_eq!(transcripts.len(), 1);
        assert_eq!(
            transcripts.read(Provider::Claude, "source").unwrap(),
            original
        );
        assert!(transcripts.read(Provider::Codex, "destination").is_err());
    }
}
