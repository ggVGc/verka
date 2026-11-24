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
//! restored row that was still live is therefore [`InteractionActivity::Stopped`]
//! with [`InteractionActivityReason::ServerRestarted`] — which is both what is
//! true and what tells the operator that resuming the Session is what would
//! bring the agent back — or [`InteractionActivityReason::ServerRestartedMidTurn`]
//! when its journal shows the restart cut a turn off. A row that had already stopped keeps its own reason:
//! the restart took nothing from it.
//! Completion is not one of the things a restart can overwrite this way: it is
//! a property of the Session (see [`crate::protocol::SessionSummary::completed`]),
//! not a reason a row is stopped, so it is read fresh off the Session's stored
//! metadata every time the roster is opened rather than carried in the
//! mirrored row at all.
//!
//! The mirrored row is the summary itself rather than a key to rebuild one
//! from, with the Driva policy beside it. Most of a summary could be
//! re-derived from the Session's stored metadata, but the Driva policy could
//! not: it is what the launch actually resolved against this host, and
//! re-deriving it would answer a different question — what a launch *now*
//! would get — while claiming to describe the run that happened. It is kept
//! beside the summary rather than in it because a summary is what every list
//! carries, and the policy is only read for the one row a client shows.

use crate::protocol::{
    CompletionState, DrivaOptions, InteractionActivity, InteractionActivityReason,
    InteractionQuery, InteractionSummary,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const ROSTER_FILE: &str = "roster.jsonl";

/// One mirrored row: the summary a client is shown, the Driva policy its run
/// was launched under, and the Session directory behind it. The path is kept beside the summary because it is what makes a
/// restored row readable — its history is replayed straight out of that
/// directory — and because a row whose Session has since been deleted can then
/// be dropped on the way in with a stat, rather than by scanning every
/// Workspace for an id that is no longer anywhere.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    session_path: PathBuf,
    summary: InteractionSummary,
    /// Absent from a row mirrored while the summary still carried it; [`read`]
    /// lifts it out of the summary for those.
    #[serde(default)]
    driva: DrivaOptions,
}

/// One row as this run hands it to [`Roster::publish`].
pub type Row = (PathBuf, InteractionSummary, DrivaOptions);

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
                // Recover older rosters' missing message time, and messages
                // journaled after the last roster snapshot, once at startup.
                if let Ok(at_ms) = crate::journal::last_user_message_at_ms(&entry.session_path) {
                    if at_ms != 0 {
                        entry.summary.last_user_message_at_ms = at_ms;
                    }
                }
                // A row already stopped lost no agent to the restart, so it
                // keeps the reason it stopped for. One that was live is asked
                // of its journal whether a turn was cut off: the mirrored
                // activity is as stale as the row's last publish. A journal
                // that cannot be read is taken as idle — the restart is
                // still the true reason, only less specific.
                if entry.summary.activity != InteractionActivity::Stopped {
                    entry.summary.activity = InteractionActivity::Stopped;
                    entry.summary.activity_reason = Some(
                        if crate::journal::ends_mid_turn(&entry.session_path).unwrap_or(false) {
                            InteractionActivityReason::ServerRestartedMidTurn
                        } else {
                            InteractionActivityReason::ServerRestarted
                        },
                    );
                }
                entry.summary.restored = true;
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
                entry.summary.completed =
                    crate::journal::read_session_completed(&entry.session_path)
                        .unwrap_or(CompletionState::Active);
                // A row mirrored before this was reported has none; the
                // Session's own record has it, and it is set once, so the
                // record is right whichever run wrote the row.
                if entry.summary.branched_from.is_none() {
                    entry.summary.branched_from =
                        crate::journal::read_session_checkout(&entry.session_path)
                            .ok()
                            .flatten()
                            .and_then(|checkout| checkout.branched_from);
                }
                // Where a Session was branched from never changes, but a row
                // mirrored before summaries carried it does not say.
                if entry.summary.origin.is_none() {
                    entry.summary.origin = crate::journal::session_summary_at(
                        &entry.session_path,
                        &entry.summary.workspace_id,
                    )
                    .ok()
                    .and_then(|summary| summary.origin);
                }
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

    /// The rows a previous run left that `query` admits, in no particular
    /// order; the caller sorts them together with its live ones.
    ///
    /// The query is applied to the mirrored row before its summary is
    /// refreshed from the Session, since that refresh reads several files per
    /// row and most rows are finished ones a polling client did not ask for.
    pub fn restored(&self, query: &InteractionQuery) -> Vec<InteractionSummary> {
        self.lock()
            .values()
            .filter(|entry| query.admits_summary(&entry.summary))
            .map(current_summary)
            .collect()
    }

    /// Each restored row branched off another Session, with the Session it
    /// was branched off. Read from the mirrored rows alone.
    pub fn origins(&self) -> Vec<(String, String)> {
        self.lock()
            .values()
            .filter_map(|entry| {
                let origin = entry.summary.origin.as_ref()?;
                Some((entry.summary.id.clone(), origin.session_id.clone()))
            })
            .collect()
    }

    /// The restored row for `id`, the Driva policy its run was launched
    /// under, and the Session directory its history is in.
    pub fn restored_session(&self, id: &str) -> Option<Row> {
        self.lock().get(id).map(|entry| {
            (
                entry.session_path.clone(),
                current_summary(entry),
                entry.driva.clone(),
            )
        })
    }

    /// The Driva policy the restored row for `id` was launched under.
    pub fn driva(&self, id: &str) -> Option<DrivaOptions> {
        self.lock().get(id).map(|entry| entry.driva.clone())
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

    /// Record that the operator stopped a row restored from a previous
    /// server. There is no process left to signal, but `Paused` is still the
    /// more useful current answer once the operator has explicitly stopped
    /// it.
    pub fn pause(&self, id: &str) -> bool {
        let mut restored = self.lock();
        let Some(entry) = restored.get_mut(id) else {
            return false;
        };
        entry.summary.activity = InteractionActivity::Stopped;
        entry.summary.activity_reason = Some(InteractionActivityReason::Paused);
        entry.summary.activity_since_ms = crate::journal::now_ms();
        true
    }

    /// List a row this run has no agent for, as a restored one is listed:
    /// a Session just branched off another, stopped until the operator
    /// resumes it. It is mirrored with the rest, so a branch left unstarted
    /// is still beside its source after a restart.
    pub fn adopt(&self, session_path: PathBuf, summary: InteractionSummary, driva: DrivaOptions) {
        self.lock().insert(
            summary.id.clone(),
            Entry {
                session_path,
                summary,
                driva,
            },
        );
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
    pub fn publish(&self, live: Vec<Row>) {
        let Some(path) = &self.path else {
            return;
        };
        let mut entries: Vec<Entry> = live
            .into_iter()
            .map(|(session_path, mut summary, driva)| {
                // The Session metadata is the sole owner of its mutable name.
                // The roster records only the Interaction snapshot and joins
                // the current name when a client asks for the row.
                summary.name = None;
                Entry {
                    session_path,
                    summary,
                    driva,
                }
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
                .cloned()
                .map(|mut entry| {
                    entry.summary.name = None;
                    entry
                }),
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

/// Join a restored Interaction's immutable run snapshot with the Session
/// metadata that still changes after the process which produced it is gone.
/// A roster written by an older build may contain a name, but it is transport
/// history rather than an authority and is deliberately ignored here.
fn current_summary(entry: &Entry) -> InteractionSummary {
    let mut summary = entry.summary.clone();
    summary.name = crate::journal::read_session_name(&entry.session_path).unwrap_or_default();
    // Tags are edited on the Session after the run that mirrored this row, so
    // the mirrored copy is stale the moment the operator retags a stopped row.
    summary.tags = crate::journal::read_session_tags(&entry.session_path).unwrap_or_default();
    // A stopped Session can switch provider without creating a new row.
    if let Ok(meta) = crate::journal::read_session_meta(&entry.session_path) {
        summary.selection = meta.selection;
    }
    // Integration can land, or finish cleaning up, after the row was mirrored.
    summary.integration = crate::journal::read_session_integration(&entry.session_path);
    // A focus request was about the launch, which a restored row is long past.
    summary.focus_requested = false;
    summary
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
        .filter_map(|mut row: serde_json::Value| {
            // A row mirrored while the summary still carried the policy has
            // it there; it is the only record of what that run launched under.
            if row.get("driva").is_none() {
                if let Some(driva) = row
                    .get_mut("summary")
                    .and_then(serde_json::Value::as_object_mut)
                    .and_then(|summary| summary.remove("driva"))
                {
                    row["driva"] = driva;
                }
            }
            serde_json::from_value(row).ok()
        })
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
mod tests;
