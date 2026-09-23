//! The roster of open Interactions, kept across a restart.
//!
//! An Interaction leaves the server's map only when the operator closes it.
//! Everything else — a turn ending, an agent exiting, a plan window refusing
//! the work — leaves it in the list, stopped, because a stopped Interaction is
//! still the conversation the operator was having and still the row they go
//! back to. A restart used to be the one exception: the map lived in memory
//! alone, so the next run started with an empty list and every open
//! conversation had to be found again among the stored Sessions.
//!
//! So the list is mirrored into the store beside the quota log, and read back
//! on the way up. What comes back is the rows, not the agents: the processes
//! died with the server that owned them, and nothing here restarts them. A
//! restored row is therefore [`InteractionActivity::Stopped`] with
//! [`InteractionActivityReason::ServerRestarted`], whatever it was doing when
//! the previous run ended — which is both what is true and what tells the
//! operator that resuming the Session is what would bring the agent back.
//! Completion is not one of the things a restart can overwrite this way: it is
//! a property of the Session (see [`crate::protocol::SessionSummary::completed`]),
//! not a reason a row is stopped, so it is read fresh off the Session's stored
//! metadata every time the roster is opened rather than carried in the
//! mirrored row at all.
//!
//! The mirrored row is the summary itself rather than a key to rebuild one
//! from. Most of a summary could be re-derived from the Session's stored
//! metadata, but the Driva policy could not: it is what the launch actually
//! resolved against this host, and re-deriving it would answer a different
//! question — what a launch *now* would get — while claiming to describe the
//! run that happened.

use crate::protocol::{
    CompletionState, InteractionActivity, InteractionActivityReason, InteractionSummary,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const ROSTER_FILE: &str = "roster.jsonl";

/// One mirrored row: the summary a client is shown, and the Session directory
/// behind it. The path is kept beside the summary because it is what makes a
/// restored row readable — its history is replayed straight out of that
/// directory — and because a row whose Session has since been deleted can then
/// be dropped on the way in with a stat, rather than by scanning every
/// Workspace for an id that is no longer anywhere.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    session_path: PathBuf,
    summary: InteractionSummary,
}

/// The open-Interaction list as the store holds it.
#[derive(Default)]
pub struct Roster {
    /// `None` for a roster that keeps nothing past the life of the process.
    path: Option<PathBuf>,
    /// Rows from a previous run that this run has not superseded. An entry
    /// leaves when the operator closes it, or when the Session is resumed and
    /// becomes a live Interaction again.
    restored: Mutex<HashMap<String, Entry>>,
}

impl Roster {
    /// The store's roster, with whatever open Interactions a previous run left
    /// behind.
    ///
    /// A missing file is the ordinary first run. An unreadable one is reported
    /// and then treated as empty: the Sessions themselves are untouched and
    /// still listed as stored Sessions, so the cost of a damaged roster is
    /// that the operator reopens their conversations by hand — not a server
    /// that refuses to start.
    pub fn open(store_root: &Path) -> Self {
        let path = store_root.join(ROSTER_FILE);
        let entries = match read(&path) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("styra-server: reopening the interaction roster: {error:#}");
                Vec::new()
            }
        };
        let restored = entries
            .into_iter()
            // A Session deleted while this roster was on disk leaves a row
            // pointing at nothing. Drop it here rather than showing the
            // operator a conversation they cannot open.
            .filter(|entry| entry.session_path.is_dir())
            .map(|mut entry| {
                entry.summary.activity = InteractionActivity::Stopped;
                entry.summary.activity_reason = Some(InteractionActivityReason::ServerRestarted);
                // Deliberately not restored, for the reason the quota log does
                // not restore its announcements: a notification is owed to the
                // operator of the run that raised it, and this is not that run.
                entry.summary.idle_unseen = false;
                // Completion lives with the Session, not the mirrored row, so
                // it is read back from there rather than trusted from disk —
                // the mirrored value is stale the moment anything else touches
                // the Session's metadata. A Session whose file has since gone
                // missing (caught by the filter above, but a race is still
                // possible) is read as not completed rather than dropping the
                // row a second time.
                entry.summary.completed = crate::journal::read_session_completed(
                    &entry.session_path,
                )
                .unwrap_or(CompletionState::Active);
                (entry.summary.id.clone(), entry)
            })
            .collect();
        Self {
            path: Some(path),
            restored: Mutex::new(restored),
        }
    }

    /// A roster that keeps nothing past the life of the process.
    pub fn new() -> Self {
        Self::default()
    }

    /// The rows a previous run left, in no particular order; the caller sorts
    /// them together with its live ones.
    pub fn restored(&self) -> Vec<InteractionSummary> {
        self.lock()
            .values()
            .map(|entry| entry.summary.clone())
            .collect()
    }

    /// The restored row for `id`, and the Session directory its history is in.
    pub fn restored_session(&self, id: &str) -> Option<(InteractionSummary, PathBuf)> {
        self.lock()
            .get(id)
            .map(|entry| (entry.summary.clone(), entry.session_path.clone()))
    }

    pub fn holds(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }

    /// Reflect a completion change already written to the Session's stored
    /// metadata onto a restored row's mirrored summary. Unlike a live
    /// interaction, it has no process left to stop; the row was stopped
    /// already, and only the flag it carries for display changes.
    pub fn set_completed(&self, id: &str, completed: CompletionState) -> bool {
        let mut restored = self.lock();
        let Some(entry) = restored.get_mut(id) else {
            return false;
        };
        entry.summary.completed = completed;
        true
    }

    /// Drop a restored row: the operator closed it, or this run has revived
    /// the Session and owns a live Interaction for it instead.
    pub fn forget(&self, id: &str) {
        self.lock().remove(id);
    }

    /// Mirror the open-Interaction list to the store: this run's live rows,
    /// plus the restored ones it has not superseded.
    ///
    /// Called when the list changes and again as the server goes down, so a
    /// graceful shutdown mirrors each row as it finally stood rather than as
    /// it stood when it was opened.
    pub fn publish(&self, live: Vec<(PathBuf, InteractionSummary)>) {
        let Some(path) = &self.path else {
            return;
        };
        let mut entries: Vec<Entry> = live
            .into_iter()
            .map(|(session_path, summary)| Entry {
                session_path,
                summary,
            })
            .collect();
        let live_ids: std::collections::HashSet<String> = entries
            .iter()
            .map(|entry| entry.summary.id.clone())
            .collect();
        entries.extend(
            self.lock()
                .values()
                .filter(|entry| !live_ids.contains(&entry.summary.id))
                .cloned(),
        );
        if let Err(error) = write(path, &entries) {
            eprintln!("styra-server: mirroring the interaction roster: {error:#}");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.restored
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Read the mirrored rows, skipping any line that no longer parses. A summary
/// gained fields over time and will again; a row this build cannot read is one
/// row lost, not a roster refused.
fn read(path: &Path) -> Result<Vec<Entry>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Publish the whole roster, atomically: the next run must never read a file
/// half-written by an Interaction that opened mid-rewrite.
fn write(path: &Path, entries: &[Entry]) -> Result<()> {
    let mut text = String::new();
    for entry in entries {
        text.push_str(&serde_json::to_string(entry).context("serializing an interaction row")?);
        text.push('\n');
    }
    let directory = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    use std::sync::atomic::{AtomicU64, Ordering};
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let temporary = directory.join(format!(
        ".{ROSTER_FILE}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        std::fs::remove_file(&temporary).ok();
        return Err(error).with_context(|| format!("publishing {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Effort, Provider, Selection};

    fn store(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "styra-roster-{}-{name}-{}",
            std::process::id(),
            crate::journal::now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn summary(id: &str) -> InteractionSummary {
        InteractionSummary {
            id: id.to_owned(),
            name: None,
            tags: Vec::new(),
            workspace_id: "workspace".into(),
            selection: Selection {
                provider: Provider::Claude,
                model: "sonnet".into(),
                effort: Effort::Medium,
            },
            workspace: PathBuf::from("/tmp/project"),
            driva: Default::default(),
            activity: InteractionActivity::Running,
            activity_reason: None,
            activity_since_ms: 7,
            idle_unseen: true,
            uncommitted_changes: false,
            checkout: Some(crate::protocol::CheckoutState {
                worktree: PathBuf::from("/tmp/worktrees/project-session"),
                repository: PathBuf::from("/tmp/project"),
                branch: Some("styra/project-session".into()),
            }),
            last_message: Some("still going".into()),
            auto_retry: false,
            events: 12,
            completed: CompletionState::Active,
        }
    }

    /// The whole point: a row mirrored by one run is a row the next run lists.
    /// It comes back stopped, because the agent behind it did not come back,
    /// and it keeps what it was talking about so the list still reads.
    #[test]
    fn a_mirrored_row_is_listed_again_by_the_next_run_as_stopped() {
        let root = store("restart");
        let session = root.join("session-a");
        std::fs::create_dir_all(&session).unwrap();

        Roster::open(&root).publish(vec![(session, summary("session-a"))]);

        let restored = Roster::open(&root).restored();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, "session-a");
        assert_eq!(restored[0].activity, InteractionActivity::Stopped);
        assert_eq!(
            restored[0].activity_reason,
            Some(InteractionActivityReason::ServerRestarted)
        );
        assert_eq!(restored[0].last_message.as_deref(), Some("still going"));
        // Where the agent was working outlives the agent. Nothing this run can
        // do would re-derive it — the branch was read from a checkout the
        // previous run's agent may since have been the last to touch — so a
        // row that lost it would have to show the operator nothing.
        let checkout = restored[0].checkout.as_ref().expect("the checkout is kept");
        assert_eq!(checkout.branch.as_deref(), Some("styra/project-session"));
        assert_eq!(checkout.repository, PathBuf::from("/tmp/project"));
        assert!(checkout.linked());
        // The previous run's notification is not this run's to raise.
        assert!(!restored[0].idle_unseen);
        std::fs::remove_dir_all(root).ok();
    }

    /// Closing an Interaction is the one thing that takes it off the list, and
    /// it has to take it off the mirrored one too — otherwise a restart would
    /// bring back precisely the conversations the operator had finished with.
    #[test]
    fn a_forgotten_row_does_not_survive_the_next_publish() {
        let root = store("forget");
        let session = root.join("session-b");
        std::fs::create_dir_all(&session).unwrap();
        Roster::open(&root).publish(vec![(session, summary("session-b"))]);

        let roster = Roster::open(&root);
        assert!(roster.holds("session-b"));
        roster.forget("session-b");
        roster.publish(Vec::new());

        assert!(Roster::open(&root).restored().is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    /// Completion is why a row is stopped, so a restart — which stops every
    /// row it brings back — must not overwrite it with its own reason.
    #[test]
    fn a_restored_row_can_be_marked_completed() {
        let root = store("complete");
        let host = store("complete-host");
        let workspace = crate::workspace::create(&root, &host, Some("work".into())).unwrap();
        let profile = crate::agent::Profile {
            name: "codex".into(),
            command: vec!["true".into()],
            protocol: crate::event::Protocol::CodexJsonl,
            mounts: Vec::new(),
            environment: Default::default(),
            network: false,
            message_format: crate::agent::MessageFormat::PlainLine,
            single_turn: false,
        };
        let (_, id) = crate::journal::Journal::create_in_workspace(
            &root,
            &workspace.id,
            &profile,
            &Selection {
                provider: Provider::Codex,
                model: "codex".into(),
                effort: Effort::Medium,
            },
            None,
        )
        .unwrap();
        let session = crate::workspace::sessions_dir(&root, &workspace.id).join(&id);
        let mut row = summary(&id);
        row.workspace_id = workspace.id.clone();
        Roster::open(&root).publish(vec![(session.clone(), row)]);

        // Set from a live interaction, as `set_completed` does: written to the
        // Session's own metadata first, then mirrored onto the roster row.
        crate::journal::store_session_completed(&session, CompletionState::Completed).unwrap();
        let roster = Roster::open(&root);
        assert!(roster.set_completed(&id, CompletionState::Completed));
        roster.publish(Vec::new());

        // Read back fresh, the way the next run does — the mirrored row is
        // never itself the record of it.
        let restored = Roster::open(&root).restored();
        assert_eq!(restored[0].completed, CompletionState::Completed);
        assert_eq!(
            restored[0].activity_reason,
            Some(InteractionActivityReason::ServerRestarted)
        );
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(host).ok();
    }

    /// A run that inherits rows and opens none of its own must not drop the
    /// inherited ones when it mirrors the list: they are still open.
    #[test]
    fn publishing_keeps_the_rows_this_run_has_not_superseded() {
        let root = store("carry");
        let first = root.join("session-c");
        let second = root.join("session-d");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        Roster::open(&root).publish(vec![
            (first.clone(), summary("session-c")),
            (second, summary("session-d")),
        ]);

        // This run revived only one of the two.
        let roster = Roster::open(&root);
        roster.forget("session-c");
        roster.publish(vec![(first, summary("session-c"))]);

        let mut ids: Vec<String> = Roster::open(&root)
            .restored()
            .into_iter()
            .map(|summary| summary.id)
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["session-c", "session-d"]);
        std::fs::remove_dir_all(root).ok();
    }

    /// A Session deleted out from under the roster leaves a row that could
    /// only be opened onto nothing.
    #[test]
    fn a_row_whose_session_is_gone_is_dropped_on_the_way_in() {
        let root = store("stale");
        let session = root.join("session-e");
        std::fs::create_dir_all(&session).unwrap();
        Roster::open(&root).publish(vec![(session.clone(), summary("session-e"))]);
        std::fs::remove_dir_all(&session).unwrap();

        assert!(Roster::open(&root).restored().is_empty());
        std::fs::remove_dir_all(root).ok();
    }
}
