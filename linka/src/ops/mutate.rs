//! Mutating operations: creating and editing nodes, recording results,
//! and the immutable observations and attachments that ride alongside them.
//!
//! Every one takes the workbench-wide mutation lock, requires a clean store,
//! and commits its complete store change as one git commit.

use super::*;
use ulid::Ulid;

pub struct InitializedWorkbench {
    pub store: Store,
    pub pairing: Pairing,
    pub created_workbench_repo: bool,
    pub created_project_repo: bool,
    pub created_project_root: bool,
}

/// Create a complete usable workbench. Frontends call this rather than
/// exposing the lower-level directory-only `Store::init` as initialization.
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

/// Parameters for creating a node with [`add`].
pub struct NewNode {
    /// The definition prose (markdown). Its first line serves as the title.
    pub description: String,
    pub author: Author,
    /// Who the work is for (e.g. `human` for a question node); `None` = anyone.
    pub assignee: Option<Author>,
    /// Ids this node depends on (must exist).
    pub depends_on: Vec<NodeId>,
    /// Ids this node is derived from (must exist).
    pub derived_from: Vec<NodeId>,
}

/// Create a new node. Returns its id.
pub fn add(store: &Store, vcs: &dyn Vcs, new: NewNode) -> Result<NodeId> {
    add_node(store, vcs, new, None)
}

/// Create a review node that verifies an exact candidate. The candidate's
/// source node is added as lineage so completing the verification pins the
/// candidate artifact through the normal result protocol.
pub fn add_verification(
    store: &Store,
    vcs: &dyn Vcs,
    candidate: &CandidateId,
    new: NewNode,
) -> Result<NodeId> {
    add_node(store, vcs, new, Some(candidate.clone()))
}

fn add_node(
    store: &Store,
    vcs: &dyn Vcs,
    mut new: NewNode,
    verifies: Option<CandidateId>,
) -> Result<NodeId> {
    if new.description.trim().is_empty() {
        bail!("a node needs a description");
    }
    let mutation = store.mutation_lock(vcs)?;
    if let Some(candidate_id) = &verifies {
        let candidate = CandidateStore::new(store).load(candidate_id)?;
        if !new.derived_from.contains(&candidate.node) {
            new.derived_from.push(candidate.node);
        }
    }
    for dep in new.depends_on.iter().chain(&new.derived_from) {
        if !store.exists(dep) {
            bail!("unknown related node `{dep}`");
        }
    }
    // A minted id is well-formed by construction; parsing it back is the only
    // way to build the validated type, and cannot fail.
    let id: NodeId = format!("node-{}", Ulid::new())
        .parse()
        .map_err(anyhow::Error::msg)?;
    let meta = NodeMeta {
        schema: DEFINITION_SCHEMA,
        author: new.author,
        assignee: new.assignee,
        depends_on: new.depends_on,
        derived_from: new.derived_from,
        verifies,
        extensions: Default::default(),
    };
    store.write_node(&id, &meta, &new.description)?;
    mutation.commit(vcs, &format!("linka: add {id}"))?;
    Ok(id)
}

/// Add `to` to one of `from`'s dependency lists. A definition change: it moves
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
    /// The submitted description matched the stored one byte-for-byte, so the
    /// node's version did not move: no commit, no reopening, no stale pins.
    Unchanged,
}

/// Edit a node's description. A definition change: it moves the node's
/// version, so a prior `done` no longer covers it and dependents' pins go
/// stale. Submitting the description a node already has is a successful no-op
/// (retries and sync-style callers converge instead of erroring).
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

/// Complete a node's work: commit all produced files as one output commit, pin
/// what the work was built against (dependency versions and outputs, plus any
/// extra context files), and record it all in `result.toml` and `result.md`.
/// Returns the output commit, or `None` when the work produced no files
/// (graph-only work).
#[allow(clippy::too_many_arguments)] // mirrors the CLI surface one-to-one
pub fn complete(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    outputs: &[String],
    context: &[String],
    message: Option<String>,
    notes: &str,
    author: Author,
) -> Result<Option<String>> {
    let outputs: Vec<String> = outputs
        .iter()
        .map(|path| {
            path.parse::<crate::model::ProjectPath>()
                .map(|path| path.to_string())
                .map_err(anyhow::Error::msg)
        })
        .collect::<Result<_>>()?;
    // Short-lived completion owns the whole store-side transaction. Establish
    // a clean, stable store before inspecting or changing the project so a
    // prior interrupted completion cannot be silently built upon and a dirty
    // store cannot leave a new project commit behind before being rejected.
    let mutation = store.mutation_lock(vcs)?;
    require_consistent_project_head(store, vcs)?;
    let (meta, description) = store.read_node(id)?;
    require_ordinary_node(&meta, id)?;
    // The only uncommitted project changes allowed are the outputs we are about
    // to commit — completion is where output provenance is asserted.
    require_clean_except(vcs, &outputs)?;

    let input_commit = vcs.head_commit()?;
    let snapshot = snapshot_work(store, vcs, id, context)?;

    let output_commit = if outputs.is_empty() {
        None
    } else {
        let message = message.unwrap_or_else(|| crate::model::title_of(&description).to_string());
        let commit_message = output_commit_message(id, message, input_commit.as_deref());
        let commit = vcs.capture(&outputs, &commit_message)?;
        vcs.retain_output(id, &commit)?;
        Some(commit)
    };

    let submitted = submit_result_locked(
        store,
        vcs,
        RecordedSubmission {
            snapshot,
            outcome: Outcome::Done.into(),
            output: output_commit
                .as_deref()
                .map(|commit| git_artifact(store, commit))
                .transpose()?,
            notes: notes.into(),
            author,
            producer: None,
        },
        Vec::new(),
        mutation,
    );
    if let Err(error) = submitted {
        if let Some(commit) = &output_commit {
            bail!(
                "inconsistent completion: project output commit {commit} was created but its \
                 Linka result was not recorded: {error}"
            );
        }
        return Err(anyhow::anyhow!(error));
    }
    Ok(output_commit)
}

/// Refuse a project checkout whose `HEAD` identifies itself as a Linka output
/// but is not recorded as that node's output in the store. This detects the
/// durable partial state left if short-lived completion is interrupted after
/// committing project outputs and before committing the Linka result.
pub fn require_consistent_project_head(store: &Store, vcs: &dyn Vcs) -> Result<()> {
    let Some(head) = vcs.head_commit()? else {
        return Ok(());
    };
    let Some(declared_node) = vcs.linka_node(&head)? else {
        return Ok(());
    };
    match origin(store, &head)? {
        Some(recorded_node) if recorded_node.as_str() == declared_node => return Ok(()),
        Some(recorded_node) => bail!(
            "inconsistent Linka state: project HEAD {} declares node `{declared_node}`, but the \
             store records it as output of `{recorded_node}`",
            short(&head)
        ),
        None => {}
    }
    declared_node
        .parse::<crate::model::NodeId>()
        .map_err(|error| {
            anyhow::anyhow!(
                "project HEAD {} has an invalid Linka-Node trailer: {error}",
                short(&head)
            )
        })?;
    if vcs.output_was_recorded(&store.store_name(), &declared_node, &head)? {
        return Ok(());
    }
    bail!(
        "inconsistent Linka state: project HEAD {} declares itself as output of node \
         `{declared_node}`, but the store has never recorded that output; restore the project \
         changes and run `linka complete` again, or move the project checkout to a \
         consistent commit",
        short(&head)
    )
}

/// Answer a node: record it done with the response as its result notes,
/// producing no output commit. Unlike [`complete`] this does not gate on
/// project-tree cleanliness: an answer asserts no output provenance, and a
/// question node is typically answered mid-work, while the tree is dirty with
/// whatever prompted the question. Dependency versions are still pinned, so
/// the answer participates in staleness like any other result.
pub fn respond(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    notes: &str,
    author: Author,
) -> Result<()> {
    if notes.trim().is_empty() {
        bail!("a response needs some text");
    }
    let snapshot = snapshot_work(store, vcs, id, &[])?;
    submit_result(
        store,
        vcs,
        ResultSubmission {
            snapshot,
            outcome: Outcome::Done,
            output: None,
            notes: notes.into(),
            author,
            producer: None,
        },
    )
    .map_err(|error| anyhow::anyhow!(error))
}

/// Record that a node's work was attempted and failed. Like [`complete`] it pins
/// what the attempt was built against, so the failure is reproducible evidence.
/// It does not gate on project-tree cleanliness: a failed attempt may well have
/// left a mess, and recording the failure must not be blocked by it.
pub fn fail(store: &Store, vcs: &dyn Vcs, id: &NodeId, notes: &str, author: Author) -> Result<()> {
    let mutation = store.mutation_lock(vcs)?;
    let (meta, _) = store.read_node(id)?;
    require_ordinary_node(&meta, id)?;
    let consumed = pin_node_list(
        store,
        &meta
            .depends_on
            .iter()
            .chain(&meta.derived_from)
            .cloned()
            .collect::<Vec<_>>(),
    )?;
    let result = ResultMeta {
        schema: RESULT_SCHEMA,
        at: now_millis(),
        author,
        definition: store.node_version(id)?,
        outcome: Outcome::Failed.into(),
        project: current_project_snapshot(store, vcs)?,
        consumed,
        context: Vec::new(),
        output: None,
        producer: None,
    };
    store.write_result(id, &result, notes)?;
    mutation.commit(vcs, &format!("linka: fail {id}"))?;
    Ok(())
}

/// Record immutable, producer-neutral context observations for one exact result.
pub fn record_context_observation(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    expected_result: &ResultVersion,
    paths: &[String],
) -> Result<usize> {
    let mutation = store.mutation_lock(vcs)?;
    let Some((result, _)) = store.read_result(id)? else {
        bail!("node `{id}` has no result");
    };
    if &store.result_version(id)? != expected_result {
        bail!("result changed before context observation was recorded");
    }

    let mut node_outputs = std::collections::HashSet::new();
    for other in store.list_ids()? {
        if let Some(commit) = output_of(store, &other)? {
            node_outputs.extend(vcs.files_in(&commit)?);
        }
    }

    let root = store.project_root();
    let mut pinned: std::collections::HashSet<String> =
        result.context.iter().map(|p| p.path.to_string()).collect();
    for observation in store.read_context_observations(id)? {
        if &observation.result == expected_result {
            pinned.extend(
                observation
                    .context
                    .into_iter()
                    .map(|pin| pin.path.to_string()),
            );
        }
    }
    let mut context = Vec::new();
    for path in paths {
        if pinned.contains(path) || node_outputs.contains(path) {
            continue;
        }
        let project_path: crate::model::ProjectPath = path.parse().map_err(anyhow::Error::msg)?;
        // Observations are discovered after execution, but their identity is
        // the content the accepted result actually ran against. Never hash a
        // possibly modified execution worktree or a checkout that has moved
        // since the attempt's frozen project snapshot.
        let frozen_revision =
            (!result.project.revision.is_empty()).then_some(result.project.revision.as_str());
        let blob = match frozen_revision {
            Some(revision) => vcs.file_blob_at(revision, project_path.as_str())?,
            None => project_file_blob(&root, &project_path)?,
        };
        let Some(blob) = blob else {
            continue;
        };
        pinned.insert(path.clone());
        context.push(ContextPin {
            path: project_path,
            identity: blob,
            observed: true,
        });
    }
    if context.is_empty() {
        return Ok(0);
    }
    let added = context.len();
    store.write_context_observation(
        id,
        &crate::model::ContextObservation {
            schema: OBSERVATION_SCHEMA,
            result: expected_result.clone(),
            context,
        },
    )?;
    mutation.commit(vcs, &format!("linka: context observation {id}"))?;
    Ok(added)
}

/// Attach arbitrary immutable data to a node and commit it to Linka's Git
/// history. Attachments are opaque to graph evaluation. Repeating the same
/// namespace/key with identical content is a no-op; changing an existing
/// attachment is refused.
pub fn record_node_attachment(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    new: NewNodeAttachment,
) -> Result<NodeAttachment> {
    Ok(record_node_attachments(store, vcs, id, vec![new])?
        .into_iter()
        .next()
        .expect("one attachment requested"))
}

/// Atomically commit several opaque attachments in one Linka mutation.
/// Existing identical items are accepted, so a caller can retry a partially
/// completed attachment batch without duplicating data or commits.
pub fn record_node_attachments(
    store: &Store,
    vcs: &dyn Vcs,
    id: &NodeId,
    new: Vec<NewNodeAttachment>,
) -> Result<Vec<NodeAttachment>> {
    if new.is_empty() {
        return Ok(Vec::new());
    }
    let mutation = store.mutation_lock(vcs)?;
    if !store.exists(id) {
        bail!("unknown node `{id}`");
    }

    let (attachments, pending) = prepare_node_attachments(store, id, new)?;
    write_node_attachments(store, id, &pending)?;
    if pending.is_empty() {
        return Ok(attachments);
    }
    mutation.commit(
        vcs,
        &format!("linka: attach {} item(s) to {id}", pending.len()),
    )?;
    Ok(attachments)
}

pub(super) fn prepare_node_attachments(
    store: &Store,
    id: &NodeId,
    new: Vec<NewNodeAttachment>,
) -> Result<(Vec<NodeAttachment>, Vec<(NodeAttachment, Vec<u8>)>)> {
    let mut identities = std::collections::HashSet::new();
    let mut attachments = Vec::with_capacity(new.len());
    let mut pending = Vec::new();
    let created_at_ms = now_millis();
    for new in new {
        if !identities.insert((new.namespace.clone(), new.key.clone())) {
            bail!("attachment batch repeats `{}/{}`", new.namespace, new.key);
        }
        if let Some((existing, data)) = store.read_node_attachment(id, &new.namespace, &new.key)? {
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
        let attachment = NodeAttachment {
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

pub(super) fn write_node_attachments(
    store: &Store,
    id: &NodeId,
    pending: &[(NodeAttachment, Vec<u8>)],
) -> Result<()> {
    for (attachment, data) in pending {
        store.write_node_attachment(id, attachment, data)?;
    }
    Ok(())
}
