use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use linka::ops::{self, SubmissionError};
use linka::{ArtifactRef, ArtifactStore, Author, GitVcs, Outcome, ResultSubmission, Store};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepared,
    ArtifactCaptured,
    ArtifactRetained,
    ResultWritten,
    StoreCommitted,
    Finalized,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubmissionJournal {
    pub schema: u32,
    pub id: String,
    pub phase: Phase,
    pub submission: ResultSubmission,
    pub output_paths: Vec<String>,
    pub output_message: String,
}

#[allow(clippy::too_many_arguments)]
pub fn complete(
    store: &Store,
    vcs: &GitVcs,
    node: &str,
    outputs: &[String],
    context: &[String],
    message: String,
    notes: String,
    author: Author,
) -> Result<Option<String>> {
    let dirty = vcs.dirty_paths()?;
    if dirty.iter().any(|path| !outputs.contains(path)) {
        bail!("project has undeclared dirty paths: {}", dirty.join(", "));
    }
    let snapshot = ops::snapshot_work(store, vcs, node, context)?;
    let id = ulid::Ulid::new().to_string();
    let mut journal = SubmissionJournal {
        schema: 1,
        id,
        phase: Phase::Prepared,
        submission: ResultSubmission {
            snapshot,
            outcome: Outcome::Done,
            output: None,
            notes,
            author,
            producer: None,
        },
        output_paths: outputs.to_vec(),
        output_message: message,
    };
    save(store, &journal)?;
    recover(store, vcs, &mut journal)?;
    Ok(journal.submission.output.map(|artifact| artifact.id))
}

pub fn recover(store: &Store, vcs: &GitVcs, journal: &mut SubmissionJournal) -> Result<()> {
    loop {
        match journal.phase {
            Phase::Prepared => {
                if !journal.output_paths.is_empty() {
                    let id = vcs.capture(&journal.output_paths, &journal.output_message)?;
                    journal.submission.output = Some(ArtifactRef {
                        scheme: "git-commit".into(),
                        repository: journal.submission.snapshot.project.repository.clone(),
                        id,
                    });
                }
                journal.phase = Phase::ArtifactCaptured;
            }
            Phase::ArtifactCaptured => {
                if let Some(output) = &journal.submission.output {
                    vcs.retain_output(journal.submission.snapshot.node.as_str(), &output.id)?;
                }
                journal.phase = Phase::ArtifactRetained;
            }
            Phase::ArtifactRetained => {
                match ops::submit_result(store, vcs, journal.submission.clone()) {
                    Ok(()) => journal.phase = Phase::ResultWritten,
                    Err(SubmissionError::Conflict(conflicts)) => {
                        save(store, journal)?;
                        bail!(
                            "submission conflict (journal {} preserved): {conflicts:?}",
                            journal.id
                        );
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            Phase::ResultWritten => journal.phase = Phase::StoreCommitted,
            Phase::StoreCommitted => journal.phase = Phase::Finalized,
            Phase::Finalized => return Ok(()),
        }
        save(store, journal)?;
    }
}

pub fn load(store: &Store, id: &str) -> Result<SubmissionJournal> {
    let path = path(store, id);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn save(store: &Store, journal: &SubmissionJournal) -> Result<()> {
    let path = path(store, &journal.id);
    std::fs::create_dir_all(path.parent().unwrap())?;
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, toml::to_string_pretty(journal)?)?;
    std::fs::rename(&temporary, &path)?;
    Ok(())
}

fn path(store: &Store, id: &str) -> PathBuf {
    journal_root(store).join(format!("{id}.toml"))
}

fn journal_root(store: &Store) -> PathBuf {
    store.workbench_root().join(".linka-submissions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_names_round_trip_and_finalized_recovery_is_idempotent() {
        let phases = [
            Phase::Prepared,
            Phase::ArtifactCaptured,
            Phase::ArtifactRetained,
            Phase::ResultWritten,
            Phase::StoreCommitted,
            Phase::Finalized,
        ];
        for phase in phases {
            let text = serde_json::to_string(&phase).unwrap();
            assert_eq!(serde_json::from_str::<Phase>(&text).unwrap(), phase);
        }
    }
}
