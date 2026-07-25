//! Orka presentation over Linka's first-class candidate protocol.
//!
//! Linka owns candidate identity, decisions, and Git-derived publication.
//! Orka adds attempt-oriented lookup and patch display, but stores no duplicate
//! candidate state and performs no publication side effect itself.

use crate::attempt::{AttemptId, AttemptRecord};
use anyhow::{bail, Context, Result};
use linka::ops::{self, NewNode};
use linka::{
    Author, CandidateId, CandidateRecord, CandidateStore, GitVcs, IntegrationStatus, Store,
    VerificationOutcome, VerificationSubmission,
};
use std::path::Path;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct Candidate {
    pub id: CandidateId,
    pub attempt: Option<AttemptId>,
    pub node: linka::NodeId,
    pub branch: String,
    pub target: String,
    pub input_commit: Option<String>,
    pub head_commit: String,
    pub integration: IntegrationStatus,
}

impl Candidate {
    pub fn status(&self) -> &'static str {
        match self.integration {
            IntegrationStatus::Pending => "pending",
            IntegrationStatus::Accepted => "accepted",
            IntegrationStatus::Published => "published",
            IntegrationStatus::Rejected => "rejected",
            IntegrationStatus::NotRequired => "direct",
        }
    }
}

pub struct Candidates<'a> {
    store: &'a Store,
}

impl<'a> Candidates<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn list(&self) -> Result<Vec<Candidate>> {
        let candidates = CandidateStore::new(self.store);
        candidates
            .list()?
            .into_iter()
            .map(|id| candidates.load(&id).and_then(|record| self.present(record)))
            .collect()
    }

    /// Resolve either Linka's candidate id or Orka's producing attempt id.
    pub fn get(&self, reference: &str) -> Result<Candidate> {
        let candidates = CandidateStore::new(self.store);
        let record = if reference.starts_with("candidate-") {
            candidates.load(&CandidateId(reference.to_string()))?
        } else {
            let external = linka::ExternalIdentity {
                namespace: "orka".into(),
                id: reference.to_string(),
            };
            let record = candidates.by_external(&external)?.with_context(|| {
                format!("no Linka candidate belongs to Orka attempt `{reference}`")
            })?;
            candidates.load(&record.id)?
        };
        self.present(record)
    }

    pub fn patch(&self, reference: &str) -> Result<String> {
        let candidate = self.get(reference)?;
        let input_commit = candidate
            .input_commit
            .as_deref()
            .context("candidate has no Orka attempt input for patching")?;
        checked(
            &self.store.project_root(),
            &[
                "diff",
                "--find-renames",
                input_commit,
                &candidate.head_commit,
            ],
        )
    }

    pub fn accept(
        &self,
        reference: &str,
        verification: &linka::NodeId,
        notes: String,
    ) -> Result<Candidate> {
        let candidate = self.get(reference)?;
        CandidateStore::new(self.store).accept(
            &GitVcs::for_store(self.store),
            &candidate.id,
            verification,
            Author::Human,
            notes,
        )?;
        self.get(&candidate.id.0)
    }

    pub fn reject(
        &self,
        reference: &str,
        verification: &linka::NodeId,
        notes: String,
    ) -> Result<Candidate> {
        let candidate = self.get(reference)?;
        CandidateStore::new(self.store).reject(
            &GitVcs::for_store(self.store),
            &candidate.id,
            verification,
            Author::Human,
            notes,
        )?;
        self.get(&candidate.id.0)
    }

    pub fn publish(&self, reference: &str) -> Result<Candidate> {
        let candidate = self.get(reference)?;
        CandidateStore::new(self.store).publish(&GitVcs::for_store(self.store), &candidate.id)?;
        self.get(&candidate.id.0)
    }

    /// Record an automated accepted verification and publish its exact candidate.
    ///
    /// The verification result goes through Linka's normal candidate-decision
    /// protocol; this does not bypass the review gate. Publication remains a
    /// separate, retryable fast-forward if it cannot complete.
    pub fn auto_accept_and_publish(&self, reference: &str) -> Result<(linka::NodeId, Candidate)> {
        let candidate = self.get(reference)?;
        let vcs = GitVcs::for_store(self.store);
        let verification: linka::NodeId = ops::add_verification(
            self.store,
            &vcs,
            &candidate.id,
            NewNode {
                description: format!(
                    "Automatically verify candidate {}\n\nSource: {}",
                    candidate.id, candidate.node
                ),
                author: Author::Machine,
                assignee: Some(Author::Machine),
                depends_on: vec![],
                derived_from: vec![],
            },
        )?
        .parse()
        .map_err(anyhow::Error::msg)?;
        let snapshot = ops::snapshot_work(self.store, &vcs, verification.as_str(), &[])?;
        ops::submit_verification(
            self.store,
            &vcs,
            VerificationSubmission {
                snapshot,
                outcome: VerificationOutcome::Accepted,
                notes: "Automatically accepted by `orka run --auto-accept`.".into(),
                author: Author::Machine,
                producer: None,
            },
        )
        .map_err(|error| match error {
            ops::SubmissionError::Conflict(conflicts) => {
                anyhow::anyhow!("automatic verification became stale: {conflicts:?}")
            }
            ops::SubmissionError::Evaluation(error) => error,
        })?;
        let published = self.publish(&candidate.id.0)?;
        Ok((verification, published))
    }

    fn present(&self, record: CandidateRecord) -> Result<Candidate> {
        let attempt = record
            .external
            .as_ref()
            .filter(|external| external.namespace == "orka")
            .map(|external| AttemptId(external.id.clone()));
        let input_commit = attempt
            .as_ref()
            .map(|attempt| {
                let key = format!("{attempt}/attempt");
                if let Some((_, data)) =
                    self.store
                        .read_node_attachment(record.node.as_str(), "orka", &key)?
                {
                    let text = std::str::from_utf8(&data)
                        .context("Orka attempt attachment is not UTF-8")?;
                    let attached: AttemptRecord =
                        toml::from_str(text).context("parsing Orka attempt attachment")?;
                    if &attached.id != attempt || attached.input.node() != &record.node {
                        bail!("Orka attempt attachment does not match its Linka candidate");
                    }
                    return Ok(Some(attached.input.input_commit().to_string()));
                }
                Ok(None)
            })
            .transpose()?
            .flatten();
        let integration = record.integration(&GitVcs::for_store(self.store))?;
        Ok(Candidate {
            id: record.id,
            attempt,
            node: record.node,
            branch: record.branch,
            target: record.target,
            input_commit,
            head_commit: record.artifact.id,
            integration,
        })
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
