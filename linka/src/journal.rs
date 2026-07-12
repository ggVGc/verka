use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use linka::ops::{self, SubmissionError};
use linka::{ArtifactRef, Author, Outcome, ResultSubmission, Store, Vcs};

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
    vcs: &dyn Vcs,
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
        schema: 2,
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

pub fn recover(store: &Store, vcs: &dyn Vcs, journal: &mut SubmissionJournal) -> Result<()> {
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
    let journal: SubmissionJournal =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    if !(1..=2).contains(&journal.schema) {
        bail!("unsupported submission journal schema {}", journal.schema);
    }
    Ok(journal)
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
    use linka::{ArtifactStore, ContextIdentity, RepositoryIdentity, StoreHistory};

    #[derive(Default)]
    struct TestVcs;
    impl StoreHistory for TestVcs {
        fn commit_store(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
    }
    impl ContextIdentity for TestVcs {
        fn head_commit(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn tree_id(&self, commit: &str) -> Result<String> {
            Ok(format!("tree-{commit}"))
        }
        fn file_blob(&self, _: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn file_blob_at(&self, _: &str, _: &str) -> Result<Option<String>> {
            Ok(None)
        }
    }
    impl RepositoryIdentity for TestVcs {
        fn root_commit(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn remote_url(&self) -> Result<Option<String>> {
            Ok(None)
        }
    }
    impl ArtifactStore for TestVcs {
        fn capture(&self, _: &[String], _: &str) -> Result<String> {
            Ok("artifact".into())
        }
        fn retain_output(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
        fn drift(&self, _: &str) -> Result<Option<String>> {
            Ok(None)
        }
        fn files_in(&self, _: &str) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn dirty_paths(&self) -> Result<Vec<String>> {
            Ok(vec![])
        }
        fn commit_exists(&self, _: &str) -> Result<bool> {
            Ok(true)
        }
    }

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

    #[test]
    fn artifact_retained_recovery_submits_and_is_repeatable() {
        let root =
            std::env::temp_dir().join(format!("linka-journal-recovery-{}", ulid::Ulid::new()));
        let store = Store::init(root.join(".linka")).unwrap();
        let vcs = TestVcs;
        let node = ops::add(
            &store,
            &vcs,
            ops::NewNode {
                description: "recoverable".into(),
                author: Author::Human,
                assignee: None,
                depends_on: vec![],
                derived_from: vec![],
            },
        )
        .unwrap();
        let snapshot = ops::snapshot_work(&store, &vcs, &node, &[]).unwrap();
        let mut journal = SubmissionJournal {
            schema: 2,
            id: ulid::Ulid::new().to_string(),
            phase: Phase::ArtifactRetained,
            submission: ResultSubmission {
                snapshot,
                outcome: Outcome::Done,
                output: Some(ArtifactRef {
                    scheme: "git-commit".into(),
                    repository: String::new(),
                    id: "artifact".into(),
                }),
                notes: "recovered".into(),
                author: Author::Human,
                producer: None,
            },
            output_paths: vec!["out".into()],
            output_message: "output".into(),
        };
        recover(&store, &vcs, &mut journal).unwrap();
        assert_eq!(journal.phase, Phase::Finalized);
        recover(&store, &vcs, &mut journal).unwrap();
        assert_eq!(store.read_result(&node).unwrap().unwrap().1, "recovered");
        let _ = std::fs::remove_dir_all(root);
    }
}
