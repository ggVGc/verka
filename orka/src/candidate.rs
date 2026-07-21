//! Orka presentation over Linka's first-class candidate protocol.
//!
//! Linka owns candidate identity, decisions, and Git-derived publication.
//! Orka adds attempt-oriented lookup and patch display, but stores no duplicate
//! candidate state and performs no publication side effect itself.

use crate::attempt::{AttemptId, AttemptRecord, FsAttemptStore};
use anyhow::{bail, Context, Result};
use linka::{
    Author, CandidateId, CandidateRecord, CandidateStore, GitVcs, IntegrationStatus, Store,
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
    attempts: &'a FsAttemptStore,
}

impl<'a> Candidates<'a> {
    pub fn new(store: &'a Store, attempts: &'a FsAttemptStore) -> Self {
        Self { store, attempts }
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
                if self.attempts.contains(attempt) {
                    return Ok(Some(
                        self.attempts
                            .load(attempt)?
                            .record
                            .input
                            .input_commit()
                            .to_string(),
                    ));
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
