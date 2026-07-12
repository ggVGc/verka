//! Review and publication policy over immutable artifacts.

use linka_core::{ArtifactRef, ResultVersion};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A candidate pins the subject node, the exact submitted result version, and
/// the immutable artifact under review. The producing attempt and the branch
/// keeping the artifact reachable are recorded so reviews can verify the
/// candidate has not moved and reworks can start from it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    pub subject: String,
    pub attempt: String,
    pub branch: String,
    pub result: ResultVersion,
    pub artifact: ArtifactRef,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Accepted,
    Rejected,
}

pub type ReviewDecision = DecisionKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeState {
    Open,
    AwaitingReview,
    Rejected,
    Integrated,
    Done,
    Failed,
}
impl NodeState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::AwaitingReview => "awaiting-review",
            Self::Rejected => "rejected",
            Self::Integrated => "integrated",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Projection required to derive review presentation state. The graph adapter
/// supplies generic core status separately, keeping this policy out of core.
pub trait ReviewStateView {
    type Error;
    fn is_review(&self, id: &str) -> Result<bool, Self::Error>;
    fn decision(&self, id: &str) -> Result<Option<ReviewDecision>, Self::Error>;
    fn integrated(&self, id: &str) -> Result<bool, Self::Error>;
    fn pending_artifact(&self, id: &str) -> Result<Option<String>, Self::Error>;
    fn latest_decision(
        &self,
        subject: &str,
        artifact: &str,
    ) -> Result<Option<ReviewDecision>, Self::Error>;
}

pub fn node_state<V: ReviewStateView>(
    view: &V,
    id: &str,
    core: linka_core::Status,
) -> NodeState {
    if view.is_review(id).unwrap_or(false) {
        return match view.decision(id).ok().flatten() {
            None => NodeState::AwaitingReview,
            Some(ReviewDecision::Rejected) => NodeState::Rejected,
            Some(ReviewDecision::Accepted) => NodeState::Integrated,
        };
    }
    if view.integrated(id).unwrap_or(false) {
        return NodeState::Integrated;
    }
    if let Some(artifact) = view.pending_artifact(id).ok().flatten() {
        return match view.latest_decision(id, &artifact).ok().flatten() {
            Some(ReviewDecision::Rejected) => NodeState::Rejected,
            _ => NodeState::AwaitingReview,
        };
    }
    match core {
        linka_core::Status::Open => NodeState::Open,
        linka_core::Status::Done => NodeState::Done,
        linka_core::Status::Failed => NodeState::Failed,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    pub candidate: String,
    pub kind: DecisionKind,
    pub notes: String,
    pub suggestion: Option<ArtifactRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishRequest {
    pub candidate: String,
    pub target: String,
    pub expected_previous: ArtifactRef,
    pub completed: bool,
}

/// Recoverable publication transaction record: prepared before the target
/// ref moves, completed after the store reflects the movement.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PublicationIntent {
    pub schema: u32,
    pub review: String,
    pub implementation: String,
    pub candidate_commit: String,
    pub target: String,
    pub target_ref: String,
    pub target_previous: String,
    pub notes: String,
    pub prepared_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

/// Portable review application messages for JSON-over-stdio adapters.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum Request {
    AddCandidate { candidate: Candidate },
    Decide { decision: Decision },
    Show { id: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", content = "value", rename_all = "snake_case")]
pub enum Response {
    Review {
        candidate: Box<Candidate>,
        decision: Option<Decision>,
    },
    Ok,
    Error(String),
}

pub fn handle_request(store: &FsCandidateStore, request: Request) -> Response {
    let result: anyhow::Result<Response> = (|| {
        Ok(match request {
            Request::AddCandidate { candidate } => {
                store.create_candidate(&candidate)?;
                Response::Ok
            }
            Request::Decide { decision } => {
                store.record_decision(&decision)?;
                Response::Ok
            }
            Request::Show { id } => Response::Review {
                candidate: Box::new(store.candidate(&id)?),
                decision: store.decision(&id)?,
            },
        })
    })();
    result.unwrap_or_else(|error| Response::Error(format!("{error:#}")))
}

pub trait CandidateStore {
    type Error;
    fn candidate(&self, id: &str) -> Result<Candidate, Self::Error>;
    fn record_decision(&self, decision: &Decision) -> Result<(), Self::Error>;
}

/// File-backed review state under the `reviews/` and `publications/`
/// namespaces this application owns. Decisions live outside core node results.
pub struct FsCandidateStore {
    root: PathBuf,
}

impl FsCandidateStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    fn dir(&self, id: &str) -> PathBuf {
        self.root.join("reviews").join(id)
    }
    fn publication_path(&self, review: &str) -> PathBuf {
        self.root
            .join("publications")
            .join(review)
            .join("publication.toml")
    }

    pub fn is_candidate(&self, id: &str) -> bool {
        self.dir(id).join("candidate.toml").is_file()
    }

    pub fn list_candidate_ids(&self) -> anyhow::Result<Vec<String>> {
        let dir = self.root.join("reviews");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    pub fn create_candidate(&self, candidate: &Candidate) -> anyhow::Result<()> {
        let dir = self.dir(&candidate.id);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("candidate.toml"),
            toml::to_string_pretty(candidate)?,
        )?;
        Ok(())
    }

    pub fn decision(&self, id: &str) -> anyhow::Result<Option<Decision>> {
        let path = self.dir(id).join("decision.toml");
        match std::fs::read_to_string(path) {
            Ok(data) => Ok(Some(toml::from_str(&data)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn write_publication(&self, publication: &PublicationIntent) -> anyhow::Result<()> {
        let path = self.publication_path(&publication.review);
        std::fs::create_dir_all(path.parent().expect("publication has parent"))?;
        std::fs::write(&path, toml::to_string_pretty(publication)?)?;
        Ok(())
    }

    pub fn read_publication(&self, review: &str) -> anyhow::Result<Option<PublicationIntent>> {
        let data = match std::fs::read_to_string(self.publication_path(review)) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(Some(toml::from_str(&data)?))
    }

    pub fn list_publication_ids(&self) -> anyhow::Result<Vec<String>> {
        let dir = self.root.join("publications");
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                ids.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }
}

impl CandidateStore for FsCandidateStore {
    type Error = anyhow::Error;
    fn candidate(&self, id: &str) -> Result<Candidate, Self::Error> {
        let data = std::fs::read_to_string(self.dir(id).join("candidate.toml"))
            .map_err(|_| anyhow::anyhow!("unknown candidate `{id}`"))?;
        Ok(toml::from_str(&data)?)
    }
    fn record_decision(&self, decision: &Decision) -> Result<(), Self::Error> {
        let candidate = self.candidate(&decision.candidate)?;
        validate_decision(&candidate, decision).map_err(anyhow::Error::msg)?;
        let dir = self.dir(&decision.candidate);
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("decision.toml"), toml::to_string_pretty(decision)?)?;
        Ok(())
    }
}

pub trait Publisher {
    type Error;
    fn publish(
        &self,
        candidate: &Candidate,
        target: &str,
        expected_previous: &ArtifactRef,
    ) -> Result<bool, Self::Error>;
}

pub struct GitPublisher {
    repository: PathBuf,
}
impl GitPublisher {
    pub fn new(repository: impl Into<PathBuf>) -> Self {
        Self {
            repository: repository.into(),
        }
    }
}
impl Publisher for GitPublisher {
    type Error = anyhow::Error;
    fn publish(
        &self,
        candidate: &Candidate,
        target: &str,
        expected: &ArtifactRef,
    ) -> Result<bool, Self::Error> {
        if candidate.artifact.scheme != "git-commit" || expected.scheme != "git-commit" {
            anyhow::bail!("Git publisher requires git-commit artifacts");
        }
        git_checked(&self.repository, &["check-ref-format", "--branch", target])?;
        let target_ref = format!("refs/heads/{target}");
        let symbolic = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repository)
            .args(["symbolic-ref", "-q", "HEAD"])
            .output()?;
        if symbolic.status.success()
            && String::from_utf8_lossy(&symbolic.stdout).trim() == target_ref
        {
            let status = git_checked(&self.repository, &["status", "--porcelain"])?;
            if !status.is_empty() {
                anyhow::bail!("target checkout is dirty; refusing publication");
            }
            if git_checked(&self.repository, &["rev-parse", "HEAD"])? != expected.id {
                return Ok(false);
            }
            return Ok(std::process::Command::new("git")
                .arg("-C")
                .arg(&self.repository)
                .args(["merge", "--ff-only", &candidate.artifact.id])
                .status()?
                .success());
        }
        Ok(std::process::Command::new("git")
            .arg("-C")
            .arg(&self.repository)
            .args([
                "update-ref",
                &target_ref,
                &candidate.artifact.id,
                &expected.id,
            ])
            .status()?
            .success())
    }
}

fn git_checked(repository: &std::path::Path, args: &[&str]) -> anyhow::Result<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repository)
        .args(args)
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}

pub fn publish_exact<P: Publisher>(
    publisher: &P,
    candidate: &Candidate,
    target: &str,
    expected_previous: &ArtifactRef,
) -> Result<(), PublishError<P::Error>> {
    match publisher.publish(candidate, target, expected_previous) {
        Ok(true) => Ok(()),
        Ok(false) => Err(PublishError::NotFastForward),
        Err(error) => Err(PublishError::Backend(error)),
    }
}

#[derive(Debug)]
pub enum PublishError<E> {
    NotFastForward,
    Backend(E),
}

pub fn validate_decision(candidate: &Candidate, decision: &Decision) -> Result<(), &'static str> {
    if candidate.id != decision.candidate {
        return Err("decision targets a different candidate");
    }
    if decision.kind == DecisionKind::Rejected && decision.notes.trim().is_empty() {
        return Err("rejection needs comments");
    }
    if decision.kind == DecisionKind::Accepted && decision.suggestion.is_some() {
        return Err("accepted candidates cannot carry suggestions");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CandidateStore;

    #[test]
    fn candidate_and_decision_round_trip_in_the_reviews_namespace() {
        let root =
            std::env::temp_dir().join(format!("linka-review-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = FsCandidateStore::new(&root);
        assert!(!store.is_candidate("review-1"));
        store
            .create_candidate(&Candidate {
                id: "review-1".into(),
                subject: "node-1".into(),
                attempt: "a".into(),
                branch: "linka/candidates/a".into(),
                result: ResultVersion {
                    metadata: "rm".into(),
                    notes: Some("rn".into()),
                },
                artifact: ArtifactRef {
                    scheme: "git-commit".into(),
                    repository: String::new(),
                    id: "commit".into(),
                },
            })
            .unwrap();
        assert!(store.is_candidate("review-1"));
        let candidate = store.candidate("review-1").unwrap();
        assert_eq!(candidate.subject, "node-1");
        assert_eq!(candidate.attempt, "a");
        assert_eq!(store.list_candidate_ids().unwrap(), vec!["review-1"]);
        store
            .record_decision(&Decision {
                candidate: "review-1".into(),
                kind: DecisionKind::Rejected,
                notes: "revise".into(),
                suggestion: None,
            })
            .unwrap();
        assert!(root.join("reviews/review-1/decision.toml").is_file());
        assert_eq!(
            store.decision("review-1").unwrap().unwrap().kind,
            DecisionKind::Rejected
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn publication_intents_round_trip_in_the_publications_namespace() {
        let root =
            std::env::temp_dir().join(format!("linka-review-pubs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = FsCandidateStore::new(&root);
        assert!(store.read_publication("review-1").unwrap().is_none());
        store
            .write_publication(&PublicationIntent {
                schema: 1,
                review: "review-1".into(),
                implementation: "node-1".into(),
                candidate_commit: "commit".into(),
                target: "main".into(),
                target_ref: "refs/heads/main".into(),
                target_previous: "base".into(),
                notes: "approved".into(),
                prepared_at: 1,
                completed_at: None,
            })
            .unwrap();
        assert_eq!(store.list_publication_ids().unwrap(), vec!["review-1"]);
        let publication = store.read_publication("review-1").unwrap().unwrap();
        assert_eq!(publication.candidate_commit, "commit");
        assert!(publication.completed_at.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn git_publisher_fast_forwards_exact_expected_target() {
        let root = std::env::temp_dir().join(format!("linka-review-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| -> String {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().into()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "test"]);
        git(&["config", "user.email", "test@example.com"]);
        std::fs::write(root.join("x"), "one").unwrap();
        git(&["add", "x"]);
        git(&["commit", "-m", "one"]);
        let old = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-b", "candidate"]);
        std::fs::write(root.join("x"), "two").unwrap();
        git(&["commit", "-am", "two"]);
        let new = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "main"]);
        let candidate = Candidate {
            id: "c".into(),
            subject: "s".into(),
            attempt: "a".into(),
            branch: "candidate".into(),
            result: ResultVersion {
                metadata: "r".into(),
                notes: None,
            },
            artifact: ArtifactRef {
                scheme: "git-commit".into(),
                repository: root.to_string_lossy().into(),
                id: new.clone(),
            },
        };
        let expected = ArtifactRef {
            scheme: "git-commit".into(),
            repository: root.to_string_lossy().into(),
            id: old,
        };
        publish_exact(&GitPublisher::new(&root), &candidate, "main", &expected).unwrap();
        assert_eq!(git(&["rev-parse", "main"]), new);
        let _ = std::fs::remove_dir_all(root);
    }

    struct FakePublisher(bool);
    impl Publisher for FakePublisher {
        type Error = &'static str;
        fn publish(
            &self,
            _candidate: &Candidate,
            _target: &str,
            _expected: &ArtifactRef,
        ) -> Result<bool, Self::Error> {
            Ok(self.0)
        }
    }

    #[test]
    fn publication_policy_works_through_publisher_interface() {
        let artifact = ArtifactRef {
            scheme: "test".into(),
            repository: "r".into(),
            id: "a".into(),
        };
        let candidate = Candidate {
            id: "c".into(),
            subject: "s".into(),
            attempt: "a".into(),
            branch: "candidate".into(),
            result: ResultVersion {
                metadata: "m".into(),
                notes: None,
            },
            artifact: artifact.clone(),
        };
        assert!(publish_exact(&FakePublisher(true), &candidate, "target", &artifact).is_ok());
        assert!(matches!(
            publish_exact(&FakePublisher(false), &candidate, "target", &artifact),
            Err(PublishError::NotFastForward)
        ));
    }
}
