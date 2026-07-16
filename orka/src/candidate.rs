//! Orka presentation over Linka's first-class candidate protocol.
//!
//! Linka owns candidate identity, decisions, and Git-derived publication.
//! Orka adds attempt-oriented lookup and patch display, but stores no duplicate
//! candidate state and performs no publication side effect itself.

use crate::attempt::AttemptId;
use anyhow::{bail, Context, Result};
use linka::{
    Author, CandidateId, CandidateStore, CandidateView, DecisionKind, GitVcs, IntegrationStatus,
    Store,
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
    pub input_commit: String,
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
            .map(|id| candidates.load(&id).and_then(|view| self.present(view)))
            .collect()
    }

    /// Resolve either Linka's candidate id or Orka's producing attempt id.
    pub fn get(&self, reference: &str) -> Result<Candidate> {
        let candidates = CandidateStore::new(self.store);
        let view = if reference.starts_with("candidate-") {
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
        self.present(view)
    }

    pub fn patch(&self, reference: &str) -> Result<String> {
        let candidate = self.get(reference)?;
        checked(
            &self.store.project_root(),
            &[
                "diff",
                "--find-renames",
                &candidate.input_commit,
                &candidate.head_commit,
            ],
        )
    }

    pub fn accept(&self, reference: &str, notes: String) -> Result<Candidate> {
        let candidate = self.get(reference)?;
        CandidateStore::new(self.store).accept(
            &GitVcs::for_store(self.store),
            &candidate.id,
            Author::Human,
            notes,
        )?;
        self.get(&candidate.id.0)
    }

    pub fn reject(&self, reference: &str, notes: String) -> Result<Candidate> {
        let candidate = self.get(reference)?;
        CandidateStore::new(self.store).reject(
            &GitVcs::for_store(self.store),
            &candidate.id,
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

    fn present(&self, view: CandidateView) -> Result<Candidate> {
        let attempt = view
            .candidate
            .external
            .as_ref()
            .filter(|external| external.namespace == "orka")
            .map(|external| AttemptId(external.id.clone()));
        let integration = view.integration(&GitVcs::for_store(self.store))?;
        let record = view.candidate;
        if let Some(decision) = &view.decision {
            if decision.kind == DecisionKind::Accepted && decision.target_ref.is_none() {
                bail!("accepted candidate `{}` has no target", record.id);
            }
        }
        Ok(Candidate {
            id: record.id,
            attempt,
            node: record.node,
            branch: record.branch,
            target: record.target,
            input_commit: record.input_commit,
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
