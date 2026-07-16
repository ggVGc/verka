//! Inspection and publication of project candidates produced by attempts.
//!
//! An attempt branch becomes a candidate when its head moves beyond the
//! frozen input commit. Durable attempt metadata ties it to its source node.

use crate::attempt::{AttemptId, FsAttemptStore, SealedState};
use crate::linka_work::LinkaWork;
use anyhow::{bail, Context, Result};
use linka::{ops, GitVcs, NodeId, Outcome, StalenessReason, Store};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub attempt: AttemptId,
    pub node: NodeId,
    pub branch: String,
    pub input_commit: String,
    pub head_commit: String,
    pub disposition: CandidateDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateDisposition {
    Publishable,
    Published,
    NotPublishable(String),
}

impl CandidateDisposition {
    pub fn label(&self) -> &str {
        match self {
            Self::Publishable => "publishable",
            Self::Published => "published",
            Self::NotPublishable(reason) => reason,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishOutcome {
    Published { commit: String },
    AlreadyPublished { commit: String },
}

pub struct Candidates<'a> {
    store: &'a Store,
    attempts: &'a FsAttemptStore,
    project: PathBuf,
}

impl<'a> Candidates<'a> {
    pub fn new(store: &'a Store, attempts: &'a FsAttemptStore) -> Self {
        Self {
            project: store.project_root(),
            store,
            attempts,
        }
    }

    /// List attempt branches that contain project work beyond their input.
    pub fn list(&self) -> Result<Vec<Candidate>> {
        let mut candidates = Vec::new();
        for id in self.attempts.list()? {
            if let Some(candidate) = self.load_candidate(&id)? {
                candidates.push(candidate);
            }
        }
        Ok(candidates)
    }

    pub fn get(&self, id: &AttemptId) -> Result<Candidate> {
        self.load_candidate(id)?
            .with_context(|| format!("attempt `{id}` produced no project candidate"))
    }

    /// Render the candidate's patch relative to its frozen project input.
    pub fn patch(&self, id: &AttemptId) -> Result<String> {
        let candidate = self.get(id)?;
        checked(
            &self.project,
            &[
                "diff",
                "--find-renames",
                &candidate.input_commit,
                &candidate.head_commit,
            ],
        )
    }

    /// Fast-forward an accepted candidate into the checked-out project branch.
    pub fn publish(&self, id: &AttemptId) -> Result<PublishOutcome> {
        let candidate = self.get(id)?;
        let snapshot = self.attempts.load(id)?;
        let expected_output = match snapshot.seal.as_ref().map(|seal| &seal.state) {
            Some(SealedState::Submitted {
                output_commit: Some(commit),
            }) => commit,
            Some(state) => bail!("candidate `{id}` is not an accepted project result ({state:?})"),
            None => bail!("candidate `{id}` belongs to an unfinished attempt"),
        };
        if candidate.head_commit != *expected_output {
            bail!(
                "candidate branch `{}` moved to {}, expected accepted output {}",
                candidate.branch,
                candidate.head_commit,
                expected_output
            );
        }

        let recorded = LinkaWork::new(self.store)
            .result_by_attempt(&candidate.node, &id.0)?
            .with_context(|| {
                format!(
                    "candidate `{id}` is no longer the recorded result for node `{}`",
                    candidate.node
                )
            })?;
        if recorded.outcome != Outcome::Done
            || recorded.output_commit.as_deref() != Some(expected_output.as_str())
        {
            bail!(
                "candidate `{id}` does not match node `{}`'s current successful result",
                candidate.node
            );
        }

        let vcs = GitVcs::for_store(self.store);
        let state = ops::node_state(self.store, &vcs, candidate.node.as_str())?;
        if state.is_complete() {
            return Ok(PublishOutcome::AlreadyPublished {
                commit: expected_output.clone(),
            });
        }
        if state
            .staleness
            .iter()
            .any(|reason| !matches!(reason, StalenessReason::OutputDrifted { .. }))
        {
            bail!(
                "node `{}` changed beyond candidate output drift; inspect `linka show {}`",
                candidate.node,
                candidate.node
            );
        }
        if !checked(&self.project, &["status", "--porcelain"])?.is_empty() {
            bail!("project checkout is dirty; commit or stash its changes before publishing");
        }
        let head = checked(&self.project, &["rev-parse", "HEAD"])?;
        if head != candidate.input_commit {
            bail!(
                "project HEAD moved from candidate input {} to {}; reconcile the candidate explicitly",
                candidate.input_commit,
                head
            );
        }
        checked(&self.project, &["symbolic-ref", "--quiet", "HEAD"])
            .context("project HEAD is detached; check out the branch to publish into")?;
        checked(&self.project, &["merge", "--ff-only", expected_output])?;

        let state = ops::node_state(self.store, &vcs, candidate.node.as_str())?;
        if !state.is_complete() {
            bail!(
                "published {} but node `{}` is still not complete; inspect `linka show {}`",
                expected_output,
                candidate.node,
                candidate.node
            );
        }
        Ok(PublishOutcome::Published {
            commit: expected_output.clone(),
        })
    }

    fn load_candidate(&self, id: &AttemptId) -> Result<Option<Candidate>> {
        let snapshot = self.attempts.load(id)?;
        let Some(workspace) = &snapshot.workspace else {
            return Ok(None);
        };
        let branch_ref = format!("refs/heads/{}", workspace.branch);
        let head = checked(
            &self.project,
            &["rev-parse", "--verify", &format!("{branch_ref}^{{commit}}")],
        )
        .with_context(|| {
            format!(
                "attempt `{id}` candidate branch `{}` is missing",
                workspace.branch
            )
        })?;
        if head == workspace.input_commit {
            return Ok(None);
        }
        let node = snapshot.record.input.node().clone();
        let disposition = match snapshot.seal.as_ref().map(|seal| &seal.state) {
            Some(SealedState::Submitted {
                output_commit: Some(output),
            }) if output == &head => {
                let vcs = GitVcs::for_store(self.store);
                let state = ops::node_state(self.store, &vcs, node.as_str())?;
                if state.is_complete() {
                    CandidateDisposition::Published
                } else if LinkaWork::new(self.store)
                    .result_by_attempt(&node, &id.0)?
                    .is_some()
                    && state
                        .staleness
                        .iter()
                        .all(|reason| matches!(reason, StalenessReason::OutputDrifted { .. }))
                {
                    CandidateDisposition::Publishable
                } else {
                    CandidateDisposition::NotPublishable("superseded-or-node-changed".into())
                }
            }
            Some(SealedState::StaleAtSubmit { .. }) => {
                CandidateDisposition::NotPublishable("stale-at-submit".into())
            }
            Some(state) => CandidateDisposition::NotPublishable(format!("{:?}", state)),
            None => CandidateDisposition::NotPublishable("unfinished".into()),
        };
        Ok(Some(Candidate {
            attempt: id.clone(),
            node,
            branch: workspace.branch.clone(),
            input_commit: workspace.input_commit.clone(),
            head_commit: head,
            disposition,
        }))
    }
}

fn checked(base: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .output()
        .with_context(|| format!("failed to run `git {}`", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "`git {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}
