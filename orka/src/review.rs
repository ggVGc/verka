//! Coordination between Linka verification nodes and Git-only Nota reviews.
//!
//! Nota sees only a repository, an exact subject commit, and a review branch.
//! Linka sees only a candidate, an ordinary verification node, and opaque
//! producer evidence. Orka owns the immutable binding and frozen snapshot that
//! let those independent records form one recoverable workflow.

use anyhow::{bail, Context, Result};
use linka::ops::{self, NewNode, SubmissionError};
use linka::{
    Author, CandidateId, CandidateRecord, CandidateStore, GitVcs, NodeId, Outcome,
    ProducerEvidence, ResultSubmission, Store, SubmissionConflict, WorkSnapshot,
};
use nota::{GitProvider, Review, StartedReview};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const REVIEW_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub schema: u32,
    pub candidate: CandidateId,
    pub verification: NodeId,
    pub branch: String,
    pub subject: String,
    pub snapshot: WorkSnapshot,
}

#[derive(Clone, Debug)]
pub struct Started {
    pub record: ReviewRecord,
    pub review: StartedReview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewVerdict {
    Approved,
    ChangesRequested,
    Commented,
}

impl ReviewVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::ChangesRequested => "changes_requested",
            Self::Commented => "commented",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FinishOutcome {
    Submitted,
    AlreadySubmitted,
    Conflict(Vec<SubmissionConflict>),
}

pub struct Reviews<'a> {
    linka: &'a Store,
    orka_root: PathBuf,
}

impl<'a> Reviews<'a> {
    pub fn new(linka: &'a Store, orka_root: impl Into<PathBuf>) -> Self {
        Self {
            linka,
            orka_root: orka_root.into(),
        }
    }

    /// Create the Linka verification and durable binding before creating the
    /// Nota branch. If branch creation is interrupted, [`resume`] performs the
    /// remaining idempotent Git step from the recorded facts. Starting a
    /// second review for the same candidate resumes its existing binding.
    pub fn start(&self, candidate: &CandidateId, assignee: Author) -> Result<Started> {
        let candidate_record = CandidateStore::new(self.linka).load(candidate)?;
        require_git_candidate(&candidate_record)?;
        let _start_lock = self.start_lock()?;
        if let Some(record) = self.for_candidate(candidate)? {
            return self.resume(&record.verification);
        }
        let vcs = GitVcs::for_store(self.linka);
        let verification: NodeId = ops::add_verification(
            self.linka,
            &vcs,
            candidate,
            NewNode {
                description: format!(
                    "Verify candidate {}\n\nSource: {}",
                    candidate_record.id, candidate_record.node
                ),
                author: Author::Human,
                assignee: Some(assignee),
                depends_on: vec![],
                derived_from: vec![],
            },
        )?
        .parse()
        .map_err(anyhow::Error::msg)?;
        let snapshot = ops::snapshot_work(self.linka, &vcs, verification.as_str(), &[])?;
        require_candidate_snapshot(&candidate_record, &snapshot)?;
        let record = ReviewRecord {
            schema: REVIEW_SCHEMA,
            candidate: candidate.clone(),
            branch: format!("nota/{verification}"),
            subject: candidate_record.artifact.id,
            verification,
            snapshot,
        };
        self.create_record(&record)?;
        let verification = record.verification.clone();
        self.start_nota(record).with_context(|| {
            format!(
                "verification was recorded; retry with `orka review resume {}`",
                verification
            )
        })
    }

    /// Create a missing Nota branch, or return the already-valid branch.
    pub fn resume(&self, verification: &NodeId) -> Result<Started> {
        let record = self.load(verification)?;
        self.validate_binding(&record)?;
        if let Ok(review) = nota::load_review_ref(&self.linka.project_root(), &record.branch) {
            return Ok(Started {
                review: StartedReview {
                    branch: review.branch,
                    marker: review.marker,
                    subject: review.subject,
                    repository: self.linka.project_root(),
                },
                record,
            });
        }
        self.start_nota(record)
    }

    pub fn load(&self, verification: &NodeId) -> Result<ReviewRecord> {
        let path = self.record_path(verification);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("unknown or unreadable review `{verification}`"))?;
        let record: ReviewRecord =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        if record.schema != REVIEW_SCHEMA {
            bail!(
                "review `{verification}` uses unsupported schema {}",
                record.schema
            );
        }
        if &record.verification != verification {
            bail!("review path and recorded verification id disagree");
        }
        Ok(record)
    }

    pub fn review(&self, verification: &NodeId) -> Result<(ReviewRecord, Review)> {
        let record = self.load(verification)?;
        self.validate_binding(&record)?;
        let review = nota::load_review_ref(&self.linka.project_root(), &record.branch)?;
        if review.subject != record.subject {
            bail!(
                "Nota review subject {} does not match candidate artifact {}",
                review.subject,
                record.subject
            );
        }
        Ok((record, review))
    }

    pub fn finish(
        &self,
        verification: &NodeId,
        verdict: ReviewVerdict,
        summary: Option<&str>,
        author: Author,
    ) -> Result<FinishOutcome> {
        let (record, review) = self.review(verification)?;
        let head = review
            .entries
            .last()
            .map(|entry| entry.commit.as_str())
            .unwrap_or(&review.marker)
            .to_string();
        if let Some((result, _)) = self.linka.read_result(verification.as_str())? {
            if matching_result(&result.producer, &record, verdict, &head) {
                return Ok(FinishOutcome::AlreadySubmitted);
            }
            bail!("verification `{verification}` already has a different result");
        }
        let notes = summary
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| render_summary(&review, verdict, &head));
        let producer = review_evidence(&record, &review, verdict, &head);
        let vcs = GitVcs::for_store(self.linka);
        match ops::submit_result(
            self.linka,
            &vcs,
            ResultSubmission {
                snapshot: record.snapshot,
                outcome: Outcome::Done,
                output: None,
                notes,
                author,
                producer: Some(producer),
            },
        ) {
            Ok(()) => Ok(FinishOutcome::Submitted),
            Err(SubmissionError::Conflict(conflicts)) => Ok(FinishOutcome::Conflict(conflicts)),
            Err(SubmissionError::Evaluation(error)) => Err(error),
        }
    }

    fn start_nota(&self, record: ReviewRecord) -> Result<Started> {
        let provider = GitProvider::new(self.linka.project_root());
        let review = nota::start_review(&provider, &record.subject, Some(&record.branch))?;
        Ok(Started { record, review })
    }

    fn validate_binding(&self, record: &ReviewRecord) -> Result<()> {
        let candidate = CandidateStore::new(self.linka).load(&record.candidate)?;
        require_git_candidate(&candidate)?;
        if candidate.artifact.id != record.subject {
            bail!("review subject does not match its Linka candidate");
        }
        if record.snapshot.node != record.verification {
            bail!("review snapshot names a different verification node");
        }
        let (meta, _) = self.linka.read_node(record.verification.as_str())?;
        if meta.verifies.as_ref() != Some(&record.candidate) {
            bail!("verification node no longer names the recorded candidate");
        }
        require_candidate_snapshot(&candidate, &record.snapshot)
    }

    fn create_record(&self, record: &ReviewRecord) -> Result<()> {
        let dir = self.review_dir(&record.verification);
        if dir.exists() {
            bail!("review `{}` already exists", record.verification);
        }
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("review.toml");
        let text = toml::to_string_pretty(record)
            .with_context(|| format!("serialising {}", path.display()))?;
        write_atomic(&path, text.as_bytes())
    }

    fn for_candidate(&self, candidate: &CandidateId) -> Result<Option<ReviewRecord>> {
        let root = self.orka_root.join("reviews");
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
        };
        let mut matches = Vec::new();
        for entry in entries {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let Ok(verification) = entry.file_name().to_string_lossy().parse::<NodeId>() else {
                continue;
            };
            if !self.record_path(&verification).is_file() {
                continue;
            }
            let record = self.load(&verification)?;
            if &record.candidate == candidate {
                matches.push(record);
            }
        }
        matches.sort_by(|a, b| a.verification.as_str().cmp(b.verification.as_str()));
        match matches.len() {
            0 => Ok(None),
            1 => Ok(matches.pop()),
            count => {
                let ids = matches
                    .iter()
                    .map(|record| record.verification.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                bail!(
                    "candidate `{candidate}` already has {count} Orka reviews ({ids}); refusing to create another"
                )
            }
        }
    }

    fn start_lock(&self) -> Result<fs::File> {
        fs::create_dir_all(&self.orka_root)
            .with_context(|| format!("creating {}", self.orka_root.display()))?;
        let path = self.orka_root.join(".review-start.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening review start lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                bail!("another review start is in progress ({})", path.display())
            }
            Err(fs::TryLockError::Error(error)) => Err(error)
                .with_context(|| format!("acquiring review start lock {}", path.display())),
        }
    }

    fn review_dir(&self, verification: &NodeId) -> PathBuf {
        self.orka_root.join("reviews").join(verification.as_str())
    }

    fn record_path(&self, verification: &NodeId) -> PathBuf {
        self.review_dir(verification).join("review.toml")
    }
}

fn require_git_candidate(candidate: &CandidateRecord) -> Result<()> {
    if candidate.artifact.scheme != "git-commit" {
        bail!(
            "candidate `{}` uses unsupported `{}` artifacts",
            candidate.id,
            candidate.artifact.scheme
        );
    }
    Ok(())
}

fn require_candidate_snapshot(candidate: &CandidateRecord, snapshot: &WorkSnapshot) -> Result<()> {
    let pin = snapshot
        .lineage
        .iter()
        .find(|pin| pin.id == candidate.node)
        .context("verification snapshot does not contain its candidate source")?;
    if pin.result.as_ref() != Some(&candidate.result)
        || pin.output.as_ref() != Some(&candidate.artifact)
    {
        bail!(
            "candidate `{}` is no longer the current source result; refusing to start or resume a review against different work",
            candidate.id
        );
    }
    Ok(())
}

fn render_summary(review: &Review, verdict: ReviewVerdict, head: &str) -> String {
    let mut lines = vec![
        format!("Review verdict: {}", verdict.as_str()),
        format!("Nota branch: {}", review.branch),
        format!("Nota head: {head}"),
        format!("Entries: {}", review.entries.len()),
    ];
    for entry in &review.entries {
        let summary = entry.message.lines().next().unwrap_or_default();
        lines.push(format!("- {:?}: {summary}", entry.kind));
    }
    lines.join("\n")
}

fn review_evidence(
    record: &ReviewRecord,
    review: &Review,
    verdict: ReviewVerdict,
    head: &str,
) -> ProducerEvidence {
    ProducerEvidence {
        namespace: "orka.nota".into(),
        data: serde_json::json!({
            "candidate": record.candidate.0,
            "verification": record.verification.as_str(),
            "branch": review.branch,
            "marker": review.marker,
            "head": head,
            "verdict": verdict.as_str(),
        }),
    }
}

fn matching_result(
    producer: &Option<ProducerEvidence>,
    record: &ReviewRecord,
    verdict: ReviewVerdict,
    head: &str,
) -> bool {
    let Some(producer) = producer else {
        return false;
    };
    producer.namespace == "orka.nota"
        && producer.data.get("verification").and_then(|v| v.as_str())
            == Some(record.verification.as_str())
        && producer.data.get("branch").and_then(|v| v.as_str()) == Some(record.branch.as_str())
        && producer.data.get("head").and_then(|v| v.as_str()) == Some(head)
        && producer.data.get("verdict").and_then(|v| v.as_str()) == Some(verdict.as_str())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("no parent directory for {}", path.display()))?;
    let temp = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&temp, bytes).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("committing {}", path.display()))
}
