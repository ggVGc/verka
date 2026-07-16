//! First-class candidate outputs and recoverable target-branch publication.
//!
//! Candidates are attached to an exact node result and immutable artifact.
//! They are not work nodes: rejected alternatives therefore do not become
//! dependencies or prevent unrelated graph settlement. Producer metadata is
//! opaque, keeping execution drivers outside Linka's domain.

use crate::model::{
    ArtifactRef, Author, IntegrationStatus, NodeId, ProducerEvidence, ResultVersion,
};
use crate::{Store, Vcs};
use anyhow::{bail, Context, Result};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CANDIDATE_SCHEMA: u32 = 1;
pub const DECISION_SCHEMA: u32 = 1;
pub const PUBLICATION_SCHEMA: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CandidateId(pub String);

impl CandidateId {
    pub fn new() -> Self {
        Self(format!("candidate-{}", ulid::Ulid::new()))
    }
}

impl Default for CandidateId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for CandidateId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub namespace: String,
    pub id: String,
}

/// Immutable identity of one proposed project output.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    pub schema: u32,
    pub id: CandidateId,
    pub created_at_ms: i64,
    pub node: NodeId,
    pub result: ResultVersion,
    pub artifact: ArtifactRef,
    /// Candidate branch name, without `refs/heads/`.
    pub branch: String,
    pub input_commit: String,
    /// Intended target branch name, without `refs/heads/`.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<ExternalIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub producer: Option<ProducerEvidence>,
}

pub struct NewCandidate {
    pub node: NodeId,
    pub branch: String,
    pub input_commit: String,
    pub target: String,
    pub external: Option<ExternalIdentity>,
    pub producer: Option<ProducerEvidence>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Accepted,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDecision {
    pub schema: u32,
    pub decided_at_ms: i64,
    pub kind: DecisionKind,
    pub author: Author,
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_previous: Option<String>,
}

/// Journal written before moving the project ref and completed afterward.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationRecord {
    pub schema: u32,
    pub candidate: CandidateId,
    pub candidate_commit: String,
    pub target_ref: String,
    pub target_previous: String,
    pub prepared_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct CandidateView {
    pub candidate: CandidateRecord,
    pub decision: Option<CandidateDecision>,
    pub publication: Option<PublicationRecord>,
}

impl CandidateView {
    pub fn integration(&self) -> IntegrationStatus {
        if self
            .publication
            .as_ref()
            .and_then(|publication| publication.completed_at_ms)
            .is_some()
        {
            return IntegrationStatus::Published;
        }
        match self.decision.as_ref().map(|decision| decision.kind) {
            None => IntegrationStatus::Pending,
            Some(DecisionKind::Accepted) => IntegrationStatus::Accepted,
            Some(DecisionKind::Rejected) => IntegrationStatus::Rejected,
        }
    }
}

pub struct CandidateStore<'a> {
    store: &'a Store,
}

impl<'a> CandidateStore<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }

    pub fn register(&self, vcs: &dyn Vcs, new: NewCandidate) -> Result<CandidateRecord> {
        validate_external(new.external.as_ref())?;
        validate_branch_name(&new.branch)?;
        validate_branch_name(&new.target)?;
        let mutation = self.store.mutation_lock(vcs)?;
        if let Some(external) = &new.external {
            if let Some(existing) = self.by_external(external)? {
                if existing.node != new.node
                    || existing.branch != new.branch
                    || existing.input_commit != new.input_commit
                    || existing.target != new.target
                {
                    bail!(
                        "external candidate identity `{}/{}` is already attached to different facts",
                        external.namespace,
                        external.id
                    );
                }
                return Ok(existing);
            }
        }

        let (result, _) = self
            .store
            .read_result(new.node.as_str())?
            .with_context(|| format!("node `{}` has no successful result to register", new.node))?;
        if result.outcome != crate::Outcome::Done {
            bail!("node `{}` does not have a successful result", new.node);
        }
        let artifact = result
            .output
            .clone()
            .with_context(|| format!("node `{}` result has no project output", new.node))?;
        let branch_ref = branch_ref(&new.branch);
        if vcs.ref_commit(&branch_ref)?.as_deref() != Some(artifact.id.as_str()) {
            bail!(
                "candidate branch `{}` does not point to recorded output {}",
                new.branch,
                artifact.id
            );
        }
        let result_version = self.store.result_version(new.node.as_str())?;
        let candidate = CandidateRecord {
            schema: CANDIDATE_SCHEMA,
            id: CandidateId::new(),
            created_at_ms: now_millis(),
            node: new.node,
            result: result_version,
            artifact,
            branch: new.branch,
            input_commit: new.input_commit,
            target: new.target,
            external: new.external,
            producer: new.producer,
        };
        write_toml(&self.record_path(&candidate.id), &candidate)?;
        mutation.commit(vcs, &format!("linka: register candidate {}", candidate.id))?;
        Ok(candidate)
    }

    pub fn list(&self) -> Result<Vec<CandidateId>> {
        let root = self.root();
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("reading {}", root.display())),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let text = entry.file_name().to_string_lossy().into_owned();
                if text.starts_with("candidate-") {
                    ids.push(CandidateId(text));
                }
            }
        }
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(ids)
    }

    pub fn load(&self, id: &CandidateId) -> Result<CandidateView> {
        validate_candidate_id(id)?;
        let candidate: CandidateRecord = read_toml(&self.record_path(id))
            .with_context(|| format!("unknown or unreadable candidate `{id}`"))?;
        if candidate.schema != CANDIDATE_SCHEMA {
            bail!(
                "candidate `{id}` uses unsupported schema {}",
                candidate.schema
            );
        }
        Ok(CandidateView {
            candidate,
            decision: read_optional(&self.decision_path(id))?,
            publication: read_optional(&self.publication_path(id))?,
        })
    }

    pub fn for_node(&self, node: &NodeId) -> Result<Vec<CandidateView>> {
        self.list()?
            .into_iter()
            .map(|id| self.load(&id))
            .filter(|view| match view {
                Ok(view) => view.candidate.node == *node,
                Err(_) => true,
            })
            .collect()
    }

    pub fn for_result(
        &self,
        node: &NodeId,
        result: &ResultVersion,
        artifact: &ArtifactRef,
    ) -> Result<Option<CandidateView>> {
        let mut matches = self
            .for_node(node)?
            .into_iter()
            .filter(|view| view.candidate.result == *result && view.candidate.artifact == *artifact)
            .collect::<Vec<_>>();
        Ok(matches.pop())
    }

    pub fn by_external(&self, external: &ExternalIdentity) -> Result<Option<CandidateRecord>> {
        for id in self.list()? {
            let candidate = self.load(&id)?.candidate;
            if candidate.external.as_ref() == Some(external) {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }

    pub fn accept(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
        author: Author,
        notes: String,
    ) -> Result<CandidateDecision> {
        let mutation = self.store.mutation_lock(vcs)?;
        let view = self.load(id)?;
        if let Some(decision) = view.decision {
            if decision.kind == DecisionKind::Accepted {
                return Ok(decision);
            }
            bail!("candidate `{id}` was already rejected");
        }
        self.require_candidate_ref(vcs, &view.candidate)?;
        self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Pending)?;
        let target_ref = branch_ref(&view.candidate.target);
        let target_previous = vcs
            .ref_commit(&target_ref)?
            .with_context(|| format!("target branch `{}` does not exist", view.candidate.target))?;
        let decision = CandidateDecision {
            schema: DECISION_SCHEMA,
            decided_at_ms: now_millis(),
            kind: DecisionKind::Accepted,
            author,
            notes,
            target_ref: Some(target_ref),
            target_previous: Some(target_previous),
        };
        write_toml(&self.decision_path(id), &decision)?;
        mutation.commit(vcs, &format!("linka: accept candidate {id}"))?;
        Ok(decision)
    }

    pub fn reject(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
        author: Author,
        notes: String,
    ) -> Result<CandidateDecision> {
        if notes.trim().is_empty() {
            bail!("rejection requires notes");
        }
        let mutation = self.store.mutation_lock(vcs)?;
        let view = self.load(id)?;
        if let Some(decision) = view.decision {
            if decision.kind == DecisionKind::Rejected && decision.notes == notes {
                return Ok(decision);
            }
            bail!("candidate `{id}` already has a different decision");
        }
        self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Pending)?;
        let decision = CandidateDecision {
            schema: DECISION_SCHEMA,
            decided_at_ms: now_millis(),
            kind: DecisionKind::Rejected,
            author,
            notes,
            target_ref: None,
            target_previous: None,
        };
        write_toml(&self.decision_path(id), &decision)?;
        mutation.commit(vcs, &format!("linka: reject candidate {id}"))?;
        Ok(decision)
    }

    pub fn publish(&self, vcs: &dyn Vcs, id: &CandidateId) -> Result<PublicationRecord> {
        let view = self.load(id)?;
        let decision = view
            .decision
            .as_ref()
            .filter(|decision| decision.kind == DecisionKind::Accepted)
            .with_context(|| format!("candidate `{id}` has not been accepted"))?;
        if let Some(publication) = &view.publication {
            if publication.completed_at_ms.is_some() {
                return Ok(publication.clone());
            }
        }
        self.require_candidate_ref(vcs, &view.candidate)?;
        self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Accepted)?;
        if view.publication.is_none() {
            let mutation = self.store.mutation_lock(vcs)?;
            let locked = self.load(id)?;
            if locked.publication.is_none() {
                let publication = PublicationRecord {
                    schema: PUBLICATION_SCHEMA,
                    candidate: id.clone(),
                    candidate_commit: view.candidate.artifact.id.clone(),
                    target_ref: decision
                        .target_ref
                        .clone()
                        .context("acceptance has no target")?,
                    target_previous: decision
                        .target_previous
                        .clone()
                        .context("acceptance has no target baseline")?,
                    prepared_at_ms: now_millis(),
                    completed_at_ms: None,
                };
                write_toml(&self.publication_path(id), &publication)?;
                mutation.commit(vcs, &format!("linka: prepare publication {id}"))?;
            }
        }
        self.recover_publication(vcs, id)
    }

    pub fn recover_publication(
        &self,
        vcs: &dyn Vcs,
        id: &CandidateId,
    ) -> Result<PublicationRecord> {
        let view = self.load(id)?;
        let mut publication = view
            .publication
            .context("candidate has no prepared publication")?;
        if publication.completed_at_ms.is_some() {
            return Ok(publication);
        }
        if publication.candidate_commit != view.candidate.artifact.id {
            bail!("publication no longer matches candidate `{id}`");
        }
        self.require_candidate_ref(vcs, &view.candidate)?;
        let target_now = vcs
            .ref_commit(&publication.target_ref)?
            .context("publication target disappeared")?;
        if target_now == publication.target_previous {
            self.require_current_candidate(vcs, &view.candidate, IntegrationStatus::Accepted)?;
            if !vcs.publish_fast_forward(
                &publication.target_ref,
                &publication.target_previous,
                &publication.candidate_commit,
            )? {
                bail!("candidate `{id}` cannot fast-forward its target branch");
            }
        } else if target_now != publication.candidate_commit {
            bail!(
                "publication target moved from {} to {} while candidate `{id}` was pending",
                publication.target_previous,
                target_now
            );
        }
        publication.completed_at_ms = Some(now_millis());
        let mutation = self.store.mutation_lock(vcs)?;
        if let Some(completed) = self.load(id)?.publication {
            if completed.completed_at_ms.is_some() {
                return Ok(completed);
            }
        }
        write_toml(&self.publication_path(id), &publication)?;
        mutation.commit(vcs, &format!("linka: complete publication {id}"))?;
        Ok(publication)
    }

    fn require_candidate_ref(&self, vcs: &dyn Vcs, candidate: &CandidateRecord) -> Result<()> {
        if vcs.ref_commit(&branch_ref(&candidate.branch))?.as_deref()
            != Some(candidate.artifact.id.as_str())
        {
            bail!(
                "candidate branch `{}` moved from accepted artifact {}",
                candidate.branch,
                candidate.artifact.id
            );
        }
        Ok(())
    }

    fn require_current_candidate(
        &self,
        vcs: &dyn Vcs,
        candidate: &CandidateRecord,
        expected: IntegrationStatus,
    ) -> Result<()> {
        let Some((result, _)) = self.store.read_result(candidate.node.as_str())? else {
            bail!("candidate `{}` source result disappeared", candidate.id);
        };
        if self.store.result_version(candidate.node.as_str())? != candidate.result
            || result.output.as_ref() != Some(&candidate.artifact)
        {
            bail!(
                "candidate `{}` is no longer the current result for node `{}`",
                candidate.id,
                candidate.node
            );
        }
        let current = self
            .for_result(&candidate.node, &candidate.result, &candidate.artifact)?
            .with_context(|| format!("candidate `{}` is no longer current", candidate.id))?;
        let state = crate::ops::node_state(self.store, vcs, candidate.node.as_str())?;
        if current.candidate.id != candidate.id
            || current.integration() != expected
            || state.currency != crate::Currency::Current
        {
            bail!(
                "candidate `{}` is not the current {:?} candidate for node `{}`",
                candidate.id,
                expected,
                candidate.node
            );
        }
        Ok(())
    }

    fn root(&self) -> PathBuf {
        self.store.root().join("candidates")
    }

    fn dir(&self, id: &CandidateId) -> PathBuf {
        self.root().join(&id.0)
    }

    fn record_path(&self, id: &CandidateId) -> PathBuf {
        self.dir(id).join("candidate.toml")
    }

    fn decision_path(&self, id: &CandidateId) -> PathBuf {
        self.dir(id).join("decision.toml")
    }

    fn publication_path(&self, id: &CandidateId) -> PathBuf {
        self.dir(id).join("publication.toml")
    }
}

fn validate_candidate_id(id: &CandidateId) -> Result<()> {
    if !id.0.starts_with("candidate-")
        || id.0.contains('/')
        || id.0.contains('\\')
        || id.0.chars().any(char::is_control)
    {
        bail!("invalid candidate id `{id}`");
    }
    Ok(())
}

fn validate_branch_name(branch: &str) -> Result<()> {
    if branch.is_empty()
        || branch.starts_with("refs/")
        || branch.contains("..")
        || branch.contains(' ')
        || branch.chars().any(char::is_control)
    {
        bail!("invalid branch name `{branch}`");
    }
    Ok(())
}

fn validate_external(external: Option<&ExternalIdentity>) -> Result<()> {
    if let Some(external) = external {
        if external.namespace.trim().is_empty() || external.id.trim().is_empty() {
            bail!("external candidate identity needs a namespace and id");
        }
    }
    Ok(())
}

fn branch_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("candidate record has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let text =
        toml::to_string_pretty(value).with_context(|| format!("serialising {}", path.display()))?;
    let temporary = parent.join(format!(
        ".{}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("committing {}", path.display()))
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

fn read_optional<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .with_context(|| format!("parsing {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{self, NewNode};
    use crate::vcs::FakeVcs;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn successful_output() -> (TempDir, Store, FakeVcs, NodeId, String) {
        let root = std::env::temp_dir().join(format!("linka-candidate-test-{}", ulid::Ulid::new()));
        let store = Store::init(root.join(".linka")).unwrap();
        let mut vcs = FakeVcs {
            root: Some("base".into()),
            next_id: "output".into(),
            ..Default::default()
        };
        vcs.commits
            .borrow_mut()
            .extend(["base".into(), "output".into()]);
        let node: NodeId = ops::add(
            &store,
            &vcs,
            NewNode {
                description: "candidate work".into(),
                author: Author::Human,
                assignee: None,
                depends_on: vec![],
                derived_from: vec![],
            },
        )
        .unwrap()
        .parse()
        .unwrap();
        ops::complete(
            &store,
            &vcs,
            node.as_str(),
            &["out.txt".into()],
            &[],
            None,
            "produced",
            Author::Machine,
        )
        .unwrap();
        vcs.refs
            .get_mut()
            .insert("refs/heads/candidates/a".into(), "output".into());
        vcs.refs
            .get_mut()
            .insert("refs/heads/main".into(), "base".into());
        vcs.drift_for.insert("output".into(), "A out.txt".into());
        (TempDir(root), store, vcs, node, "output".into())
    }

    fn register(store: &Store, vcs: &FakeVcs, node: &NodeId) -> CandidateRecord {
        CandidateStore::new(store)
            .register(
                vcs,
                NewCandidate {
                    node: node.clone(),
                    branch: "candidates/a".into(),
                    input_commit: "base".into(),
                    target: "main".into(),
                    external: Some(ExternalIdentity {
                        namespace: "test-runner".into(),
                        id: "run-1".into(),
                    }),
                    producer: None,
                },
            )
            .unwrap()
    }

    #[test]
    fn candidate_acceptance_and_publication_are_first_class_node_state() {
        let (_temp, store, vcs, node, output) = successful_output();
        assert_eq!(
            ops::node_state(&store, &vcs, node.as_str())
                .unwrap()
                .currency,
            crate::Currency::Stale,
            "without a candidate this is a direct output drift"
        );

        let candidate = register(&store, &vcs, &node);
        let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
        assert_eq!(state.currency, crate::Currency::Current);
        assert_eq!(state.integration, IntegrationStatus::Pending);
        assert!(!state.is_ready());
        assert!(!state.is_complete());

        let candidates = CandidateStore::new(&store);
        candidates
            .accept(&vcs, &candidate.id, Author::Human, "looks good".into())
            .unwrap();
        assert_eq!(
            ops::node_state(&store, &vcs, node.as_str())
                .unwrap()
                .integration,
            IntegrationStatus::Accepted
        );
        let publication = candidates.publish(&vcs, &candidate.id).unwrap();
        assert!(publication.completed_at_ms.is_some());
        assert_eq!(vcs.refs.borrow().get("refs/heads/main"), Some(&output));
        let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
        assert_eq!(state.integration, IntegrationStatus::Published);
        assert!(state.is_complete());

        // External identity and publication are both idempotent.
        assert_eq!(register(&store, &vcs, &node).id, candidate.id);
        assert_eq!(
            candidates.publish(&vcs, &candidate.id).unwrap(),
            publication
        );
    }

    #[test]
    fn rejection_returns_the_source_node_to_ready_without_losing_the_candidate() {
        let (_temp, store, vcs, node, _) = successful_output();
        let candidate = register(&store, &vcs, &node);
        CandidateStore::new(&store)
            .reject(&vcs, &candidate.id, Author::Human, "needs changes".into())
            .unwrap();
        let state = ops::node_state(&store, &vcs, node.as_str()).unwrap();
        assert_eq!(state.integration, IntegrationStatus::Rejected);
        assert!(state.is_ready());
        assert_eq!(
            CandidateStore::new(&store).for_node(&node).unwrap().len(),
            1
        );
    }

    #[test]
    fn a_moved_source_cannot_accept_an_obsolete_candidate() {
        let (_temp, store, vcs, node, _) = successful_output();
        let candidate = register(&store, &vcs, &node);
        ops::edit(&store, &vcs, node.as_str(), "candidate work changed".into()).unwrap();
        let error = CandidateStore::new(&store)
            .accept(&vcs, &candidate.id, Author::Human, String::new())
            .unwrap_err();
        assert!(error.to_string().contains("not the current"), "{error:#}");
    }

    #[test]
    fn recovery_finishes_when_the_target_moved_before_store_finalization() {
        let (_temp, store, vcs, node, output) = successful_output();
        let candidate = register(&store, &vcs, &node);
        let candidates = CandidateStore::new(&store);
        let decision = candidates
            .accept(&vcs, &candidate.id, Author::Human, String::new())
            .unwrap();
        let publication = PublicationRecord {
            schema: PUBLICATION_SCHEMA,
            candidate: candidate.id.clone(),
            candidate_commit: output.clone(),
            target_ref: decision.target_ref.unwrap(),
            target_previous: decision.target_previous.unwrap(),
            prepared_at_ms: 1,
            completed_at_ms: None,
        };
        write_toml(&candidates.publication_path(&candidate.id), &publication).unwrap();
        vcs.refs
            .borrow_mut()
            .insert("refs/heads/main".into(), output);

        let recovered = candidates.recover_publication(&vcs, &candidate.id).unwrap();
        assert!(recovered.completed_at_ms.is_some());
    }
}
