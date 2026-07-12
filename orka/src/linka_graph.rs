//! The production [`WorkGraph`] adapter over the Linka library.
//!
//! All graph access goes through Linka's public operations — never its
//! on-disk representation. Freezing uses the same pinning completion uses, so
//! a submission is accepted exactly when Linka's version-checked completion
//! agrees the graph has not moved.

use crate::ports::{
    ArtifactPin, DefinitionFingerprint, FrozenDependency, FrozenInput, NodeId, ResultFingerprint,
    SubmitOutcome, Submission, WorkGraph, WorkItem, WorkOutcome,
};
use anyhow::{bail, Context, Result};
use linka::{ops, Author, GitVcs, Store, Vcs};
use std::path::{Path, PathBuf};

pub struct LinkaWorkGraph {
    store: Store,
}

impl LinkaWorkGraph {
    /// Open the graph store at `store_root` (e.g. `<workbench>/.linka`).
    pub fn open(store_root: PathBuf) -> Result<Self> {
        Ok(Self {
            store: Store::open(store_root)?,
        })
    }

    /// The project directory results and outputs resolve against.
    pub fn project_root(&self) -> PathBuf {
        self.store.project_root()
    }

    fn vcs(&self) -> GitVcs {
        GitVcs::for_store(&self.store)
    }

    /// Project-side operations run in the attempt's execution worktree when
    /// one is given; graph state still commits to the workbench repository.
    fn vcs_at(&self, workspace: Option<&Path>) -> GitVcs {
        match workspace {
            Some(tree) => GitVcs::for_execution(&self.store, tree.to_path_buf()),
            None => self.vcs(),
        }
    }
}

impl WorkGraph for LinkaWorkGraph {
    fn select_ready(&self) -> Result<Vec<WorkItem>> {
        let vcs = self.vcs();
        let mut items = Vec::new();
        // Machine-assignable work only: nodes assigned to a human are theirs.
        for id in ops::ready_nodes(&self.store, &vcs, Some(Author::Machine))? {
            let (_, description) = self.store.read_node(&id)?;
            items.push(WorkItem {
                id: NodeId(id),
                title: linka::title_of(&description).to_string(),
            });
        }
        Ok(items)
    }

    fn freeze(&self, id: &NodeId) -> Result<FrozenInput> {
        let vcs = self.vcs();
        let state = ops::node_state(&self.store, &vcs, &id.0)?;
        if !state.is_ready() {
            bail!("node `{id}` is not ready to be worked");
        }
        let (_, description) = self.store.read_node(&id.0)?;
        let definition = self.store.node_version(&id.0)?;
        let input_commit = vcs
            .head_commit()?
            .context("the project repository has no commits to anchor an attempt to")?;

        let mut dependencies = Vec::new();
        for pin in ops::pin_dependencies(&self.store, &id.0)? {
            let (_, dep_description) = self.store.read_node(&pin.id)?;
            let result_notes = self
                .store
                .read_result(&pin.id)?
                .map(|(_, notes)| notes)
                .unwrap_or_default();
            dependencies.push(FrozenDependency {
                id: NodeId(pin.id),
                definition: definition_fingerprint(&pin.definition),
                result: pin.result.map(|r| ResultFingerprint {
                    metadata: r.metadata,
                    notes: r.notes,
                }),
                output: pin.output.map(|a| ArtifactPin {
                    scheme: a.scheme,
                    repository: a.repository,
                    id: a.id,
                }),
                title: linka::title_of(&dep_description).to_string(),
                result_notes,
            });
        }

        Ok(FrozenInput {
            node: id.clone(),
            definition: definition_fingerprint(&definition),
            description,
            dependencies,
            input_commit,
        })
    }

    fn submit(&self, submission: &Submission) -> Result<SubmitOutcome> {
        let id = &submission.frozen.node.0;
        let expected = expected_input(&submission.frozen);
        match &submission.outcome {
            WorkOutcome::Succeeded {
                outputs,
                message,
                notes,
            } => {
                let vcs = self.vcs_at(submission.workspace.as_deref());
                match ops::complete_checked(
                    &self.store,
                    &vcs,
                    id,
                    outputs,
                    &[],
                    message.clone(),
                    notes,
                    Author::Machine,
                    &expected,
                )? {
                    ops::CheckedCompletion::Accepted { output_commit } => {
                        Ok(SubmitOutcome::Accepted { output_commit })
                    }
                    ops::CheckedCompletion::Stale { reasons } => {
                        Ok(SubmitOutcome::Stale { reasons })
                    }
                }
            }
            WorkOutcome::Failed { notes } => {
                // Failure is evidence, not completion, so readiness is not
                // required — but evidence pins current versions, so it is
                // only faithful while the graph still matches the freeze.
                let reasons = ops::verify_frozen(&self.store, id, &expected)?;
                if !reasons.is_empty() {
                    return Ok(SubmitOutcome::Stale { reasons });
                }
                let vcs = self.vcs_at(submission.workspace.as_deref());
                ops::fail(&self.store, &vcs, id, notes, Author::Machine)?;
                Ok(SubmitOutcome::Accepted {
                    output_commit: None,
                })
            }
        }
    }
}

fn definition_fingerprint(version: &linka::DefinitionVersion) -> DefinitionFingerprint {
    DefinitionFingerprint {
        metadata: version.metadata.clone(),
        description: version.description.clone(),
    }
}

fn expected_input(frozen: &FrozenInput) -> ops::ExpectedInput {
    ops::ExpectedInput {
        definition: linka::DefinitionVersion {
            metadata: frozen.definition.metadata.clone(),
            description: frozen.definition.description.clone(),
        },
        consumed: frozen
            .dependencies
            .iter()
            .map(|d| linka::ConsumedNode {
                id: d.id.0.clone(),
                definition: linka::DefinitionVersion {
                    metadata: d.definition.metadata.clone(),
                    description: d.definition.description.clone(),
                },
                result: d.result.as_ref().map(|r| linka::ResultVersion {
                    metadata: r.metadata.clone(),
                    notes: r.notes.clone(),
                }),
                output: d.output.as_ref().map(|a| linka::ArtifactRef {
                    scheme: a.scheme.clone(),
                    repository: a.repository.clone(),
                    id: a.id.clone(),
                }),
            })
            .collect(),
    }
}
