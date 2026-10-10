//! Durable action auditing. All requests pass through one exhaustive scope
//! classification before dispatch. Adding a request therefore requires an
//! explicit decision about its action history; its payload is serialized whole.
//!
//! One store-wide ledger avoids copying workspace changes into many journals.
//! Each invocation records the affected Sessions at the time it runs. The
//! interaction endpoint selects their records, including starts whose Session
//! was only known once creation finished. Resuming keeps the same Session id.

use crate::protocol::{Action, ActionOrigin, ActionRecord, ActionStatus, Request};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(crate) enum Scope<'a> {
    Read,
    Store,
    Session(&'a str),
    Workspace(&'a str),
    All,
}

/// Intentionally no wildcard: new operations cannot silently evade auditing.
pub(crate) fn scope(request: &Request) -> Scope<'_> {
    match request {
        Request::Health
        | Request::ListWorkspaces
        | Request::Workspace { .. }
        | Request::WorkspaceForPath { .. }
        | Request::WorkspaceLaunch { .. }
        | Request::PlanSession(_)
        | Request::ListTemplates { .. }
        | Request::ListModels
        | Request::ListWorktrees { .. }
        | Request::ListTags
        | Request::SessionComposer { .. }
        | Request::Updates { .. }
        | Request::ListInteractions
        | Request::ListSessions { .. }
        | Request::StoredSession { .. }
        | Request::ProviderRaw { .. }
        | Request::TurnAnswer { .. }
        | Request::QuotaLog
        | Request::InteractionActions { .. } => Scope::Read,
        Request::CreateSession(request) => match request.checkout_from.as_deref() {
            Some(id) => Scope::Session(id),
            None => Scope::Store,
        },
        Request::ResumeSession(request) => Scope::Session(&request.id),
        Request::RenameWorkspace(request) => Scope::Workspace(&request.id),
        Request::SetWorkspaceGitRepository { workspace_id, .. }
        | Request::SetWorkspaceHostPath { workspace_id, .. }
        | Request::ChangeWorkspaceLaunch { workspace_id, .. } => Scope::Workspace(workspace_id),
        Request::RenameSession(request) => Scope::Session(&request.id),
        Request::SetSessionTags(request) => Scope::Session(&request.id),
        Request::CreateSessionWorktree { id }
        | Request::ConvertSessionProvider { id }
        | Request::BranchSession { id, .. }
        | Request::SetSessionComposer { id, .. }
        | Request::SendMessage { id, .. }
        | Request::SetSessionSelection { id, .. }
        | Request::SetInteractionWorkingDirectory { id, .. }
        | Request::SetInteractionAutoRetry { id, .. }
        | Request::SetInteractionAutoCommit { id, .. }
        | Request::QueueMessage { id, .. }
        | Request::SendQueuedMessage { id }
        | Request::ClearQueuedMessages { id }
        | Request::InterruptInteraction { id }
        | Request::StopInteraction { id }
        | Request::SetSessionCompleted { id, .. }
        | Request::CloseInteraction { id }
        | Request::LoadInteraction { id }
        | Request::Shell { id } => Scope::Session(id),
        Request::CleanWorktrees {
            workspace_id: Some(id),
        } => Scope::Workspace(id),
        Request::CleanWorktrees { workspace_id: None } | Request::Shutdown => Scope::All,
        Request::CreateWorkspace(_)
        | Request::TranscribeAudio { .. }
        | Request::AudioRecordingStarted
        | Request::AudioRecordingStopped
        | Request::AudioTranscriptionError { .. } => Scope::Store,
    }
}

pub(crate) struct ActionLog {
    path: PathBuf,
    writer: Mutex<()>,
}

impl ActionLog {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            path: root.join("actions.jsonl"),
            writer: Mutex::new(()),
        }
    }

    fn append(&self, record: &ActionRecord) -> Result<()> {
        let _writer = self.writer.lock().expect("action log lock poisoned");
        std::fs::create_dir_all(self.path.parent().expect("action log has a parent"))?;
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("opening action log {}", self.path.display()))?;
        discard_incomplete_tail(&mut file)?;
        let mut line = serde_json::to_vec(record)?;
        line.push(b'\n');
        file.write_all(&line).context("writing action log")?;
        file.sync_data().context("persisting action log")
    }

    /// Persist intent before executing a mutation. Failure prevents execution.
    pub(crate) fn start(
        &self,
        action: Action,
        origin: ActionOrigin,
        sessions: Vec<String>,
    ) -> Result<ActionRecord> {
        let record = ActionRecord {
            id: uuid::Uuid::new_v4().to_string(),
            at_ms: crate::journal::now_ms(),
            sessions,
            action,
            origin,
            status: ActionStatus::Started,
            detail: None,
        };
        self.append(&record)?;
        Ok(record)
    }

    pub(crate) fn finish(
        &self,
        mut record: ActionRecord,
        status: ActionStatus,
        detail: Option<String>,
        sessions: Vec<String>,
    ) -> Result<()> {
        record.at_ms = crate::journal::now_ms();
        record.status = status;
        record.detail = detail;
        record.sessions.extend(sessions);
        record.sessions.sort();
        record.sessions.dedup();
        self.append(&record)
    }

    pub(crate) fn for_session(&self, session: &str) -> Result<Vec<ActionRecord>> {
        // Also serialize with append: a reader must not see a partial line.
        let _writer = self.writer.lock().expect("action log lock poisoned");
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("reading action log"),
        };
        let mut reader = BufReader::new(file);
        let mut records: Vec<ActionRecord> = Vec::new();
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line)? != 0 {
            // A crash may leave a partially written final record. Only newline
            // terminated records were committed; append repairs this tail.
            if line.last() != Some(&b'\n') {
                break;
            }
            records.push(serde_json::from_slice(&line).context("parsing action log record")?);
            line.clear();
        }
        let ids: HashSet<&str> = records
            .iter()
            .filter(|record| record.sessions.iter().any(|id| id == session))
            .map(|record| record.id.as_str())
            .collect();
        let selected = records
            .iter()
            .filter(|record| ids.contains(record.id.as_str()))
            .cloned()
            .collect();
        Ok(selected)
    }
}

/// Recover only an unterminated tail, never hide corruption in a complete
/// record. Scan backwards in blocks, so ordinary appends only read one byte.
fn discard_incomplete_tail(file: &mut File) -> Result<()> {
    let mut end = file.metadata()?.len();
    if end == 0 {
        return Ok(());
    }
    file.seek(SeekFrom::Start(end - 1))?;
    let mut last = [0];
    file.read_exact(&mut last)?;
    if last[0] == b'\n' {
        return Ok(());
    }
    let mut buffer = [0; 4096];
    while end > 0 {
        let start = end.saturating_sub(buffer.len() as u64);
        let block = &mut buffer[..(end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(block)?;
        if let Some(index) = block.iter().rposition(|byte| *byte == b'\n') {
            file.set_len(start + index as u64 + 1)?;
            return Ok(());
        }
        end = start;
    }
    file.set_len(0)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_links_its_intent_and_survives_reopening_without_linking_unrelated_sessions() {
        let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
        let log = ActionLog::new(&root);
        let request = Request::StopInteraction {
            id: "source".into(),
        };
        let started = log
            .start(
                Action::Request(request.clone()),
                ActionOrigin::Client,
                vec![],
            )
            .unwrap();
        log.finish(
            started,
            ActionStatus::Succeeded,
            None,
            vec!["new-session".into()],
        )
        .unwrap();
        let failed = log
            .start(
                Action::Request(request.clone()),
                ActionOrigin::Automatic,
                vec!["new-session".into()],
            )
            .unwrap();
        log.finish(
            failed,
            ActionStatus::Failed,
            Some("provider refused".into()),
            vec![],
        )
        .unwrap();
        let reopened = ActionLog::new(&root);
        let records = reopened.for_session("new-session").unwrap();
        assert_eq!(records.len(), 4);
        assert_eq!(records[0].status, ActionStatus::Started);
        assert_eq!(records[1].status, ActionStatus::Succeeded);
        assert_eq!(records[2].origin, ActionOrigin::Automatic);
        assert_eq!(records[3].detail.as_deref(), Some("provider refused"));
        assert_eq!(records[0].action, Action::Request(request));
        assert!(reopened.for_session("unrelated").unwrap().is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_crash_tail_does_not_hide_history_or_corrupt_the_next_append() {
        let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
        let log = ActionLog::new(&root);
        let started = log
            .start(
                Action::Request(Request::Shutdown),
                ActionOrigin::Client,
                vec!["session".into()],
            )
            .unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join("actions.jsonl"))
            .unwrap();
        file.write_all(b"{\"id\":\"unfinished").unwrap();
        assert_eq!(log.for_session("session").unwrap(), vec![started.clone()]);
        log.finish(started, ActionStatus::Succeeded, None, vec![])
            .unwrap();
        let records = log.for_session("session").unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].status, ActionStatus::Succeeded);
        file.write_all(b"invalid complete record\n").unwrap();
        assert!(
            log.for_session("session").is_err(),
            "complete corruption must be surfaced"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_actions_keep_complete_correlated_records() {
        let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
        let log = std::sync::Arc::new(ActionLog::new(&root));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let log = log.clone();
                std::thread::spawn(move || {
                    let started = log
                        .start(
                            Action::Request(Request::StopInteraction {
                                id: "session".into(),
                            }),
                            ActionOrigin::Client,
                            vec!["session".into()],
                        )
                        .unwrap();
                    log.finish(started, ActionStatus::Succeeded, None, vec![])
                        .unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let records = log.for_session("session").unwrap();
        assert_eq!(records.len(), 16);
        let ids: HashSet<_> = records.iter().map(|record| &record.id).collect();
        assert_eq!(ids.len(), 8);
        for id in ids {
            let pair: Vec<_> = records.iter().filter(|record| &record.id == id).collect();
            assert_eq!(pair.len(), 2);
            assert_eq!(pair[0].status, ActionStatus::Started);
            assert_eq!(pair[1].status, ActionStatus::Succeeded);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failed_intent_write_prevents_an_action_from_starting() {
        let root = std::env::temp_dir().join(format!("styra-actions-{}", uuid::Uuid::new_v4()));
        std::fs::write(&root, "not a directory").unwrap();
        let log = ActionLog::new(&root);
        assert!(log
            .start(
                Action::Request(Request::Shutdown),
                ActionOrigin::Client,
                vec![]
            )
            .is_err());
        std::fs::remove_file(root).unwrap();
    }
}
