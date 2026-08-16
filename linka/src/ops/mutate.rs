//! The fact writers: creating and editing nodes, registering candidates, and
//! recording the observations and attachments that ride alongside a result.
//!
//! Every one takes the mutation lock, requires a clean store, and produces
//! exactly one git commit.

use super::*;
use crate::model::{
    Attachment, Candidate, CandidateId, DepKind, NewAttachment, NewCandidate, NodeMeta,
    ObservedContext, Outcome, ResultVersion, ATTACHMENT_SCHEMA, CANDIDATE_SCHEMA,
    DEFINITION_SCHEMA, OBSERVATION_SCHEMA,
};
use crate::store::blob_id;

pub struct InitializedWorkbench {
    pub store: Store,
    pub pairing: Pairing,
    pub created_workbench_repo: bool,
    pub created_project_repo: bool,
    pub created_project_root: bool,
}

/// Create a complete, usable workbench: the store, both repositories, and the
/// pairing between them. Frontends call this rather than the directory-only
/// `Store::init`.
pub fn init_workbench(
    root: std::path::PathBuf,
    name: Option<String>,
) -> Result<InitializedWorkbench> {
    let store = Store::init(root)?;
    let created_workbench_repo = crate::git::ensure_repo(&store.workbench_root())?;
    let created_project_repo = crate::git::ensure_repo(&store.project_root())?;
    let created_project_root = crate::git::ensure_root_commit(&store.project_root())?;
    let vcs = crate::git::GitVcs::for_store(&store);
    let pairing = pair(&store, &vcs, name, false)?;
    Ok(InitializedWorkbench {
        store,
        pairing,
        created_workbench_repo,
        created_project_repo,
        created_project_root,
    })
}

/// Record which project repository this store describes, keyed by the
/// project's root commit. Idempotent when the recorded root already matches. A
/// mismatch is the error this exists to catch — the wrong project sitting in
/// the workbench, or a rewritten history — and needs `force` to overwrite.
///
/// The two informational fields ride along for human readers and are never
/// checked: `name`, given by the caller, and the project's `origin` remote URL,
/// observed here.
pub fn pair(store: &Store, vcs: &dyn Vcs, name: Option<String>, force: bool) -> Result<Pairing> {
    let mutation = store.mutation_lock(vcs)?;
    let Some(root) = vcs.root_commit()? else {
        bail!("the project repository has no commits yet — nothing to pair to");
    };
    let remote = vcs.remote_url()?;
    if let Some(existing) = Pairing::load(store.root())? {
        if existing.root_commit == root {
            let updated = Pairing {
                name: name.or_else(|| existing.name.clone()),
                remote: remote.or_else(|| existing.remote.clone()),
                ..existing.clone()
            };
            if updated.name == existing.name && updated.remote == existing.remote {
                return Ok(existing);
            }
            updated.save(store.root())?;
            mutation.commit(vcs, "linka: pair project (update info)")?;
            return Ok(updated);
        }
        if !force {
            bail!(
                "store is paired to project root {} but the project's root is {} — \
                 wrong project in the workbench, or a rewritten history \
                 (re-pair with --force if this is intentional)",
                short(&existing.root_commit),
                short(&root)
            );
        }
    }
    let pairing = Pairing {
        schema: crate::pairing::PAIRING_SCHEMA,
        root_commit: root,
        paired_at: now_millis(),
        name,
        remote,
    };
    pairing.save(store.root())?;
    mutation.commit(vcs, "linka: pair project")?;
    Ok(pairing)
}

/// Parameters for creating a node.
pub struct NewNode {
    /// The definition prose. Its first non-empty line names the node.
    pub description: String,
    pub author: Author,
    /// Who the work is for (e.g. `human` for a question); `None` = anyone.
    pub assignee: Option<Author>,
    pub depends_on: Vec<NodeId>,
    pub derived_from: Vec<NodeId>,
}

/// Create a node, optionally as the review of an exact candidate. A review
/// node derives from the candidate's source node, so concluding it pins the
/// candidate's exact result and artifact through the ordinary protocol.
pub fn add(
    store: &Store,
    vcs: &dyn Vcs,
    mut new: NewNode,
    verifies: Option<CandidateId>,
) -> Result<NodeId> {
    if new.description.trim().is_empty() {
        bail!("a node needs a description");
    }
    let mutation = store.mutation_lock(vcs)?;
    if let Some(id) = &verifies {
        let candidate = store.read_candidate(id)?;
        if !new.derived_from.contains(&candidate.node) {
            new.derived_from.push(candidate.node);
        }
    }
    for edge in new.depends_on.iter().chain(&new.derived_from) {
        if !store.exists(edge) {
            bail!("unknown related node `{edge}`");
        }
    }
    let id = NodeId::new();
    let meta = NodeMeta {
        schema: DEFINITION_SCHEMA,
        author: new.author,
        assignee: new.assignee,
        depends_on: new.depends_on,
        derived_from: new.derived_from,
        verifies,
    };
    store.write_node(&id, &meta, &new.description)?;
    mutation.commit(vcs, &format!("linka: add {id}"))?;
    Ok(id)
}

/// Add `to` to one of `from`'s edge lists. A definition change: it moves
/// `from`'s version.
pub fn link(store: &Store, vcs: &dyn Vcs, from: &NodeId, to: &NodeId, kind: DepKind) -> Result<()> {
    if from == to {
        bail!("cannot link a node to itself");
    }
    let mutation = store.mutation_lock(vcs)?;
    if !store.exists(to) {
        bail!("unknown related node `{to}`");
    }
    let (mut meta, description) = store.read_node(from)?;
    let edges = match kind {
        DepKind::DependsOn => &mut meta.depends_on,
        DepKind::DerivedFrom => &mut meta.derived_from,
    };
    if edges.contains(to) {
        bail!("duplicate edge");
    }
    edges.push(to.clone());
    store.write_node(from, &meta, &description)?;
    mutation.commit(vcs, &format!("linka: link {from} -> {to}"))?;
    Ok(())
}

/// What an [`edit`] did: whether the description actually moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Edited,
    /// The submitted description matched the stored one byte for byte, so the
    /// version did not move: no commit, no reopening, no stale pins.
    Unchanged,
}

/// Edit a node's description. A definition change: it moves the node's
/// version, so a prior `done` no longer covers it and dependents' pins go
/// stale. Submitting the description a node already has is a successful no-op,
/// so retries and sync-style callers converge instead of erroring.
pub fn edit(store: &Store, vcs: &dyn Vcs, id: &NodeId, description: String) -> Result<EditOutcome> {
    if description.trim().is_empty() {
        bail!("a node needs a description");
    }
    let mutation = store.mutation_lock(vcs)?;
    let (meta, current) = store.read_node(id)?;
    if current == description {
        return Ok(EditOutcome::Unchanged);
    }
    store.write_node(id, &meta, &description)?;
    mutation.commit(vcs, &format!("linka: edit {id}"))?;
    Ok(EditOutcome::Edited)
}

/// Register a proposed project output for a node's current successful result.
///
/// The record is immutable and carries no decision: whether it is accepted or
/// rejected is derived from the verifications that name it. Registering the
/// same facts again returns the existing candidate, so a producer that crashed
/// mid-registration converges instead of duplicating.
pub fn register_candidate(store: &Store, vcs: &dyn Vcs, new: NewCandidate) -> Result<Candidate> {
    validate_branch_name(&new.branch)?;
    validate_branch_name(&new.target)?;
    let mutation = store.mutation_lock(vcs)?;

    let (existing, problems) = store.load_candidates()?;
    if let Some(problem) = problems.first() {
        bail!("cannot register a candidate while the candidate set is damaged: {problem}");
    }
    if let Some(external) = &new.external {
        if let Some(candidate) = existing
            .iter()
            .find(|candidate| candidate.external.as_ref() == Some(external))
        {
            if candidate.node != new.node
                || candidate.branch != new.branch
                || candidate.target != new.target
            {
                bail!(
                    "external candidate identity `{}/{}` is already attached to different facts",
                    external.namespace,
                    external.id
                );
            }
            return Ok(candidate.clone());
        }
    }

    let (result, _) = store
        .read_result(&new.node)?
        .with_context(|| format!("node `{}` has no result to propose output for", new.node))?;
    if result.outcome != Outcome::Done {
        bail!("node `{}` does not have a successful result", new.node);
    }
    let artifact = result
        .output
        .clone()
        .with_context(|| format!("node `{}` result has no project output", new.node))?;
    let version = store.result_version(&new.node)?;
    if let Some(candidate) = existing.iter().find(|candidate| {
        candidate.node == new.node && candidate.result == version && candidate.artifact == artifact
    }) {
        if candidate.branch == new.branch
            && candidate.target == new.target
            && candidate.external == new.external
        {
            return Ok(candidate.clone());
        }
        bail!(
            "node `{}` result already has candidate `{}` with different facts",
            new.node,
            candidate.id
        );
    }

    let candidate = Candidate {
        schema: CANDIDATE_SCHEMA,
        id: CandidateId::new(),
        node: new.node,
        result: version,
        artifact,
        branch: new.branch,
        target: new.target,
        external: new.external,
    };
    store.write_candidate(&candidate)?;
    mutation.commit(vcs, &format!("linka: register candidate {}", candidate.id))?;
    Ok(candidate)
}

/// Record context a node's work turned out to have read, for one exact result.
///
/// The pins take their identity from the *result's* frozen project revision,
/// never from a worktree that may have moved since. Paths already pinned, and
/// paths that are some node's recorded output, are skipped. Returns how many
/// new pins were recorded; zero writes nothing and commits nothing.
pub fn record_observed_context(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    expected: &ResultVersion,
    paths: &[String],
) -> Result<usize> {
    let mutation = store.mutation_lock(vcs)?;
    let Some((result, _)) = store.read_result(id)? else {
        bail!("node `{id}` has no result");
    };
    if store.result_version(id)? != *expected {
        bail!("result changed before the observed context was recorded");
    }

    let mut outputs = std::collections::HashSet::new();
    for other in store.node_ids()? {
        if let Some((other, _)) = store.read_result(&other)? {
            if let Some(artifact) = &other.output {
                outputs.extend(vcs.files_in(&artifact.id)?);
            }
        }
    }

    let mut pins = store
        .read_observed_context(id)?
        .filter(|observed| observed.result == *expected)
        .map(|observed| observed.pins)
        .unwrap_or_default();
    let mut pinned: std::collections::HashSet<String> = result
        .context
        .iter()
        .chain(pins.iter())
        .map(|pin| pin.path.to_string())
        .collect();

    let root = store.project_root();
    let revision =
        (!result.project.revision.is_empty()).then_some(result.project.revision.as_str());
    let mut added = 0;
    for path in paths {
        if pinned.contains(path) || outputs.contains(path) {
            continue;
        }
        let path: ProjectPath = path.parse().map_err(anyhow::Error::msg)?;
        let Some(identity) = context_blob(vcs, &root, revision, &path)? else {
            continue;
        };
        pinned.insert(path.to_string());
        pins.push(ContextPin {
            path,
            identity,
            observed: true,
        });
        added += 1;
    }
    if added == 0 {
        return Ok(0);
    }
    store.write_observed_context(
        id,
        &ObservedContext {
            schema: OBSERVATION_SCHEMA,
            result: expected.clone(),
            pins,
        },
    )?;
    mutation.commit(vcs, &format!("linka: observed context {id}"))?;
    Ok(added)
}

/// Attach immutable, opaque data to a node in one commit. Recording identical
/// data again is idempotent; changing an existing namespace/key is refused.
pub fn attach(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    new: Vec<NewAttachment>,
) -> Result<Vec<Attachment>> {
    if new.is_empty() {
        return Ok(Vec::new());
    }
    let mutation = store.mutation_lock(vcs)?;
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }
    let (attachments, pending) = prepare_attachments(store, id, new)?;
    if pending.is_empty() {
        return Ok(attachments);
    }
    write_attachments(store, id, &pending)?;
    mutation.commit(
        vcs,
        &format!("linka: attach {} item(s) to {id}", pending.len()),
    )?;
    Ok(attachments)
}

/// Every attachment in a batch, and the subset that still has to be written.
pub(super) type PreparedAttachments = (Vec<Attachment>, Vec<(Attachment, Vec<u8>)>);

/// Resolve an attachment batch against what is already stored, without writing
/// anything: the full list to report, and only the items that must be written.
pub(super) fn prepare_attachments(
    store: &Store,
    id: &NodeId,
    new: Vec<NewAttachment>,
) -> Result<PreparedAttachments> {
    let mut identities = std::collections::HashSet::new();
    let mut attachments = Vec::with_capacity(new.len());
    let mut pending = Vec::new();
    let created_at_ms = now_millis();
    for new in new {
        if !identities.insert((new.namespace.clone(), new.key.clone())) {
            bail!("attachment batch repeats `{}/{}`", new.namespace, new.key);
        }
        if let Some((existing, data)) = store.read_attachment(id, &new.namespace, &new.key)? {
            if existing.media_type != new.media_type || data != new.data {
                bail!(
                    "attachment `{}/{}` already exists with different content",
                    new.namespace,
                    new.key
                );
            }
            attachments.push(existing);
            continue;
        }
        let attachment = Attachment {
            schema: ATTACHMENT_SCHEMA,
            namespace: new.namespace,
            key: new.key,
            created_at_ms,
            media_type: new.media_type,
            content: blob_id(&new.data),
            size: new.data.len() as u64,
        };
        pending.push((attachment.clone(), new.data));
        attachments.push(attachment);
    }
    Ok((attachments, pending))
}

pub(super) fn write_attachments(
    store: &Store,
    id: &NodeId,
    pending: &[(Attachment, Vec<u8>)],
) -> Result<()> {
    for (attachment, data) in pending {
        store.write_attachment(id, attachment, data)?;
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
