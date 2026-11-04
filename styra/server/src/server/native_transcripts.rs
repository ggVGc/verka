//! Storage for provider-native transcripts, shared by switching and branching.
use super::{find_native_session_file, native_session_destination};
use crate::agent::Provider;
use anyhow::{Context, Result};
use std::path::Path;

/// Native ids belong to a provider. The working directory is needed when
/// writing because Claude scopes its transcripts by project; it is not needed
/// when reading, since providers may move transcripts within their own trees.
pub(super) trait NativeTranscripts: Send + Sync {
    fn read(&self, provider: Provider, native_id: &str) -> Result<String>;
    fn write(&self, provider: Provider, native_id: &str, cwd: &Path, text: &str) -> Result<()>;
}

pub(super) struct FilesystemTranscripts;

impl NativeTranscripts for FilesystemTranscripts {
    fn read(&self, provider: Provider, native_id: &str) -> Result<String> {
        let path = find_native_session_file(provider, native_id)?;
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))
    }

    fn write(&self, provider: Provider, native_id: &str, cwd: &Path, text: &str) -> Result<()> {
        let path = native_session_destination(provider, native_id, cwd)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}

/// Convert and store a branch of native history. Switching passes the entire
/// conversation; explicit branches may choose a cutoff or a single message.
/// Session/journal updates belong to the caller and happen after this succeeds.
pub(super) fn write_converted(
    transcripts: &dyn NativeTranscripts,
    source: &str,
    from: Provider,
    to: Provider,
    native_id: &str,
    cwd: &Path,
    point: super::BranchPoint<'_>,
) -> Result<()> {
    let converted = super::branch_native_session(
        source,
        super::native_session_format(from),
        super::native_session_format(to),
        native_id,
        cwd,
        point,
    )?;
    transcripts.write(to, native_id, cwd, &converted)
}

/// Configurable storage for workflow tests. It models provider/id lookup and
/// write failure without knowing which workflow or conversation is under test.
#[cfg(test)]
#[derive(Default)]
pub(super) struct MemoryTranscripts {
    entries: std::sync::Mutex<std::collections::BTreeMap<(String, String), String>>,
    fail_writes: std::sync::atomic::AtomicBool,
}

#[cfg(test)]
impl MemoryTranscripts {
    pub(super) fn fail_writes(&self, fail: bool) {
        self.fail_writes
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    pub(super) fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }
}

#[cfg(test)]
impl NativeTranscripts for MemoryTranscripts {
    fn read(&self, provider: Provider, native_id: &str) -> Result<String> {
        self.entries
            .lock()
            .unwrap()
            .get(&(provider.as_str().into(), native_id.into()))
            .cloned()
            .with_context(|| format!("{} session {native_id:?} does not exist", provider.as_str()))
    }

    fn write(&self, provider: Provider, native_id: &str, _cwd: &Path, text: &str) -> Result<()> {
        anyhow::ensure!(
            !self.fail_writes.load(std::sync::atomic::Ordering::Relaxed),
            "native storage unavailable"
        );
        self.entries
            .lock()
            .unwrap()
            .insert((provider.as_str().into(), native_id.into()), text.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        let text =
            genta::session::serialize(&session, super::super::native_session_format(provider));
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
}
