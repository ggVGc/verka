# Linka design

## Purpose

Linka is a standalone, git-versioned graph of work nodes. It records what work
means, how work items relate, and what results were produced. It does not run
agents, manage containers, orchestrate attempts, or interpret reviews.

Git provides history, integrity, blame, and distribution. Linka provides the
graph semantics git cannot express: what a unit of work is, what it depends on,
what evidence covers it, and whether that evidence still holds.

## Principles

Three rules decide every design question below.

1. **One writable record per fact.** A fact is stored in exactly one place. If
   two files could disagree about the same thing, one of them is wrong by
   construction and must be derived instead.
2. **State is a fold over records.** Outcome, currency, integration, and
   workability are computed from stored facts on every read. Nothing caches a
   status, and no operation "sets" one.
3. **A bad record is a state, not a query failure.** A node whose files cannot
   be read or parsed evaluates to `error`. It is never ready and it blocks its
   dependents, but it does not prevent the rest of the graph from being
   queried. Genuine I/O failure — the disk is gone, git is missing — is still an
   error return. The distinction is *this record is bad* versus *I cannot look*.

## The model

### Nodes and definitions

A node has an immutable identity and a versioned definition. The definition is
structured metadata (`node.toml`) plus prose (`description.md`); its version is
the pair of git blob ids over those exact bytes. There is no stored title — the
first non-empty line of the description names the node wherever a one-liner is
needed.

Two directed edge kinds relate nodes:

- `depends_on` — the target must be complete before this node is workable.
- `derived_from` — lineage and provenance. It records what work came out of,
  contributes to staleness, and never gates readiness.

Edges are ids only. Which *versions* a node was built against is a fact about
the work, not about the definition, so it is recorded in the result's pins at
submission time. Updating a pin therefore never counts as a definition change.

Nothing else lives in `node.toml`. In particular there is no open-ended
extension map: application data flattened into the definition would silently
move the definition version, and an unrelated tool writing a key would
invalidate every result and pin in the graph.

### Results

A node has at most one result. It pins the exact definition it covered, the
dependency and lineage versions it consumed, the context files it read, the
project revision it ran against, and the artifact it produced, if any.

A result's outcome is one of five values, in two families:

- **work** — `done`, `failed`
- **verification** — `accepted`, `rejected`, `abandoned`

Which family a node accepts is fixed by its definition: a node with `verifies`
set is a review node and takes only verification outcomes; every other node
takes only work outcomes. A result whose outcome does not match its node's kind
is a corrupt record, and the node evaluates to `error`.

Results are never overwritten as hidden mutable state. Recording a new result
replaces the file, and git history holds the previous ones.

### Candidates

A candidate is a proposed project output attached to an exact node result and
an immutable artifact commit. It exists so that alternatives can be produced
and rejected without becoming graph nodes — rejected work must not turn into a
dependency or poison graph settlement.

**A candidate record is immutable in full.** It carries its identity, source
node, source result version, artifact, display branch, intended target branch,
and an optional external identity. It carries *no* decision state: whether it
was accepted or rejected is derived from the results of the verification nodes
that name it. That is principle 1 applied to the single fact most likely to be
recorded twice.

The artifact commit is authoritative. The producer's branch is informational
and may be moved or deleted without affecting candidate validity.

The optional `external` identity is a producer-owned namespaced key. It exists
so a producer can register the same candidate twice — after a crash, say —
without creating a duplicate. Linka never interprets the namespace.

### Verification

A verification is a review node whose definition sets `verifies` to the exact
candidate under review. Its `derived_from` lineage must include that candidate's
source node, so its result pins the exact source result and artifact through the
ordinary submission protocol.

A verification cannot be completed or failed. Its result is `accepted`,
`rejected`, or `abandoned`. All three are terminal for that verification; only
`accepted` satisfies a downstream `depends_on` edge. `rejected` means the review
rejected the candidate; `abandoned` means no candidate decision was reached.

Several verification nodes may name the same candidate. The candidate's decision
is the conclusion of whichever current, non-abandoned verification decided it;
two current verifications disagreeing about one candidate is a corrupt graph,
reported by `check` and evaluated as `error` on the candidate's source node.

Review discussion, evidence, and reviewer authorization policy belong to other
applications. Linka owns the accepted/rejected/abandoned conclusion and safe
publication once an authorized caller requests it.

### Opaque application data

Applications need to keep durable evidence without adding producer-specific
concepts to the graph model. Three placements, one shape:

```rust
struct Namespaced { namespace: String, data: toml::Value }
```

- `ResultMeta.producer` — namespaced evidence about what produced a result.
- **Attachments** — namespaced, keyed, immutable bytes associated with a node.
  Linka commits the exact bytes and basic content metadata, and never
  interprets them or admits them to definition, result, readiness, or staleness
  semantics. Recording identical data again is idempotent; changing an existing
  namespace/key is refused. Several attachments may be recorded atomically in
  one mutation, including alongside a result.
- `Candidate.external` — the producer-owned idempotency key described above.

## Derived state

Graph state is derived, never stored, and has four independent dimensions.

- **Recorded outcome** — `open`, `succeeded`, or `failed` for ordinary work;
  `open`, `accepted`, `rejected`, or `abandoned` for verification.
- **Currency** — `current` or `stale` against the exact definition, consumed
  pins, context, and artifact the result recorded.
- **Integration** — `not-required`, `pending`, `accepted`, `published`, or
  `rejected`. Direct results need no integration.
- **Workability** — `complete`, `ready`, `awaiting-integration`, `blocked`, or
  `error`.

```rust
enum NodeState {
    Error { message: String },
    Known {
        outcome: RecordedOutcome,
        currency: Currency,
        integration: IntegrationStatus,
        staleness: Vec<StalenessReason>,
        blockers: Vec<Blocker>,
    },
}
```

### Rules

1. A node is **complete** exactly when it has a successful, current result whose
   integration is either not required or published.
2. A node is **awaiting integration** while its current successful candidate is
   pending a decision, or accepted but unpublished. It is not redispatched.
3. A node is **ready** when it is neither complete nor awaiting integration and
   every current `depends_on` target is complete. Rejecting the current
   candidate returns the node to ready.
4. Other valid nodes are **blocked** by incomplete `depends_on` targets.
5. A node whose own records are unreadable, unparseable, or self-inconsistent is
   in **error**. It is never ready or complete, and it blocks its dependents
   with reason `error`.
6. `derived_from` records lineage and provenance but does not gate readiness.

### Truth table

| Outcome | Currency | Integration | Dependencies | Workability |
| --- | --- | --- | --- | --- |
| open | current | not-required | complete | ready |
| failed | current or stale | not-required | complete | ready |
| succeeded | current | not-required or published | complete | complete |
| succeeded | current | pending or accepted | complete | awaiting-integration |
| succeeded | current | rejected | complete | ready |
| succeeded | stale | any | complete | ready |
| accepted, rejected, or abandoned | current | not-required | complete | complete |
| accepted, rejected, or abandoned | stale | not-required | complete | ready |
| any | any | any | incomplete or error | blocked |
| unreadable | — | — | — | error |

The rules above are the normative ones and the table summarises them, so the
rules decide where the two could be read differently. The case that matters is
a node that is itself complete whose dependency later regresses: rule 1 makes
completion a fact about a node's own result, so it stays complete and its
dependents stay complete with it. Reading the table's last-but-one row as
overriding rule 1 would instead cascade "blocked" back through settled work,
which answers a question nobody asked — whether the branch of work is still
sound is what `settled` is for, and it reports exactly that.

### Integration is derived from git

A result that declares an artifact and has a candidate takes its integration
status from the candidate's decision and the target branch's history:

| Candidate decision | Git check | Integration |
| --- | --- | --- |
| none yet | — | `pending` |
| rejected | — | `rejected` |
| accepted | `is_ancestor(artifact, target)` | `published` |
| accepted | otherwise | `accepted` |

There is no case in which reading integration fails. A candidate does not
record the target's pre-publication commit, and a target branch that has moved
is not an integrity error — it simply is or is not an ancestor. Publication
retries and reports a plain fast-forward failure rather than making the node
permanently unreadable.

A result with an artifact but no candidate has integration `not-required`; its
output was produced and applied directly.

### Staleness

Missing facts with defined semantics are not corruption. An absent result is
`open`; a context path proven absent is stale; an artifact proven absent is
stale. Failures to *read* or *parse* definitions, results, or context, and
artifact-backend failures, are errors — reported as such, never converted into
`open`, `ready`, `blocked`, or `stale`.

```rust
enum StalenessReason {
    DefinitionChanged { metadata: bool, description: bool },
    ConsumedDefinitionChanged { id: NodeId },
    ConsumedNodeMissing { id: NodeId },
    ConsumedResultChanged { id: NodeId },
    ConsumedOutputChanged { id: NodeId },
    ContextChanged { path: ProjectPath },
    ContextMissing { path: ProjectPath },
    OutputDrifted { artifact: String, detail: String },
}
```

**A result backed by a candidate never drifts.** The artifact commit is
immutable and authoritative, and absence from the target branch is integration,
not staleness. Drift checking therefore applies only to direct results, compared
against the project working tree.

### Blockers

```rust
enum BlockerReason { Missing, Open, Failed, Rejected, Abandoned,
                     Stale, AwaitingIntegration, Error }
struct Blocker { id: NodeId, reason: BlockerReason }
```

A `depends_on` cycle makes every node in it evaluate to `error` with the cycle
path as its message, using the same detector `check` reports with. There is one
behaviour for one condition.

## Storage

The store is `.linka/` in a *workbench*: an outer git repository holding the
store next to the project, which is an ordinary, entirely separate git
repository.

```text
<workbench>/            outer repo — store history
  .linka/               the store
  project/              inner repo — the actual project
```

Work sessions run inside `project/` with file tools scoped to it, so the store
sits above the granted subtree and a node's context stays what the graph says
it is, with no deny rules.

### Layout

```text
.linka/
  pairing.toml                        which project repo this store describes
  nodes/<node-id>/
    node.toml                         definition metadata
    description.md                    definition prose
    result.toml                       the one result, if any
    result.md                         result prose, if any
    observed-context.toml             post-hoc context pins for the current result
    attachments/<namespace>/<key>/
      meta.toml
      data
  candidates/<candidate-id>.toml      immutable
```

Every path component is a literal, validated name. Attachments are addressed by
their namespace and key directly rather than by a hash of them, which removes
the possibility of a record and its directory disagreeing.

Candidates are single files because they are immutable and have no children.

### Records

```rust
struct NodeMeta {
    schema: u32,
    author: Author,                  // human | machine
    assignee: Option<Author>,        // who the work is for; None = anyone
    depends_on: Vec<NodeId>,
    derived_from: Vec<NodeId>,
    verifies: Option<CandidateId>,   // present ⇒ review node
}

struct ResultMeta {
    schema: u32,
    at: i64,                         // unix milliseconds
    author: Author,
    definition: DefinitionVersion,   // the definition this result covered
    outcome: Outcome,                // done|failed|accepted|rejected|abandoned
    project: ProjectSnapshot,        // scheme, repository, revision, tree
    consumed: Vec<ConsumedNode>,     // pinned depends_on + derived_from
    context: Vec<ContextPin>,
    output: Option<ArtifactRef>,     // scheme, repository, id
    producer: Option<Namespaced>,
}

struct ConsumedNode {
    id: NodeId,
    definition: DefinitionVersion,
    result: Option<ResultVersion>,
    outcome: Option<Outcome>,
    output: Option<ArtifactRef>,
}

struct ContextPin { path: ProjectPath, identity: String, observed: bool }

struct Candidate {
    schema: u32,
    id: CandidateId,
    node: NodeId,
    result: ResultVersion,           // the exact result it proposes output for
    artifact: ArtifactRef,
    branch: String,                  // display branch, informational
    target: String,                  // intended target branch
    external: Option<ExternalIdentity>,
}
```

`observed-context.toml` holds `{ result: ResultVersion, pins: [ContextPin] }`
and is replaced wholesale. It records context discovered *after* execution —
files an agent turned out to have read — pinned by their content at the result's
frozen project revision, never by a worktree that may have moved since. Its pins
contribute to staleness exactly like the result's own. It is a separate file
because amending the result itself would move the result version and invalidate
every candidate and pin that referenced it.

### Versions

A definition version is the pair of git blob ids over `node.toml` and
`description.md`. A result version is the blob ids over `result.toml` and
`result.md`. Blob ids are computed locally — `sha1("blob <len>\0" + bytes)` —
so version identity needs no git process. This assumes git's SHA-1 object
format; a SHA-256 repository would need the hash function to follow the
repository's setting.

### Transactions

Every store mutation follows one boundary:

1. Acquire the workbench-wide mutation lock — an OS file lock on
   `<workbench>/.git/linka-mutation.lock`, so it never enters a commit and is
   released if the process exits.
2. Refuse to proceed unless the tracked `.linka/` store is clean.
3. Perform the complete action.
4. Commit it as one git commit.
5. Verify the store is clean again, and release.

Failed writes or commits may leave evidence in the working tree, and that dirty
state deliberately blocks every later mutation until explicitly resolved.
Read-only inspection remains available throughout.

### Short-lived completion

`complete` holds the mutation lock from its clean-store precondition through
result commit. It commits declared outputs in the project repository before
recording the result in the store, and retains the output ref only *after* the
submission is accepted — a rejected submission leaves no dangling Linka ref.

There is deliberately no submission journal. If recording the result fails,
Linka reports the created output commit. If the process is interrupted, the
library refuses a later completion from a project `HEAD` carrying a `Linka-Node`
trailer that has never appeared in committed store history. Previously recorded
historical outputs remain valid evidence.

## Evaluation

Derived state is computed by one memoized pass over the whole store, not by
recursion per query.

```rust
struct Graph<'a> {
    store: &'a Store,
    vcs: &'a dyn Vcs,                          // wrapped in a per-pass memo
    nodes: BTreeMap<NodeId, Result<NodeMeta, String>>,
    candidates_by_result: HashMap<(NodeId, ResultVersion), Vec<Candidate>>,
    states: RefCell<HashMap<NodeId, NodeState>>,
}

impl Graph<'_> {
    fn load(store: &Store, vcs: &dyn Vcs) -> Result<Self>;   // one scan
    fn state(&self, id: &NodeId) -> &NodeState;              // memoized
    fn ready(&self, worker: Option<Author>) -> Vec<&NodeId>;
    fn blocked(&self) -> Vec<(&NodeId, &[Blocker])>;
    fn stale(&self) -> Vec<(&NodeId, &[StalenessReason])>;
    fn settled(&self, id: &NodeId) -> Vec<String>;
    fn dependents(&self, id: &NodeId) -> Vec<&NodeId>;
    fn origin(&self, commit: &str) -> Option<&NodeId>;
}
```

Three properties matter:

- **Memoization.** Evaluation is iterative with an explicit stack and a cache
  keyed by node id, so a diamond dependency evaluates its shared ancestor once
  and deep chains cannot overflow the stack.
- **Candidates are indexed once.** Loading and parsing the candidate set is a
  single scan at construction, not a directory listing per node evaluation.
- **The VCS is memoized per pass.** `ref_commit`, `is_ancestor`, `tree_id`, and
  `drift` are pure within one evaluation, so each distinct question costs one
  subprocess regardless of how many nodes ask it.

Every listing query — ready, blocked, stale, settled, dependents, origin — is a
projection over the same pass.

## Operations

### Fact writers

Each takes the mutation lock and produces exactly one commit.

```rust
fn init_workbench(root: PathBuf, name: Option<String>) -> Result<InitializedWorkbench>;
fn pair(store, vcs, name: Option<String>, force: bool) -> Result<Pairing>;

fn add(store, vcs, new: NewNode, verifies: Option<CandidateId>) -> Result<NodeId>;
fn link(store, vcs, from: &NodeId, to: &NodeId, kind: DepKind) -> Result<()>;
fn edit(store, vcs, id: &NodeId, description: String) -> Result<EditOutcome>;

fn register_candidate(store, vcs, new: NewCandidate) -> Result<Candidate>;
fn record_observed_context(store, vcs, id, expected: &ResultVersion,
                           paths: &[String]) -> Result<usize>;
fn attach(store, vcs, id: &NodeId, new: Vec<NewAttachment>) -> Result<Vec<Attachment>>;

fn submit(store, vcs, submission: Submission) -> Result<(), SubmissionError>;
```

`submit` is the only way a result is ever written.

```rust
enum Conclusion {
    Done { output: Option<ArtifactRef> },
    Failed,
    Accepted,
    Rejected,
    Abandoned,
}

struct Submission {
    snapshot: WorkSnapshot,
    conclusion: Conclusion,
    notes: String,
    author: Author,
    producer: Option<Namespaced>,
    attachments: Vec<NewAttachment>,   // same commit; validated before any write
}
```

Answering a question is `Done { output: None }`. Recording a failed attempt is
`Failed`. Concluding a review is `Accepted`, `Rejected`, or `Abandoned` — and
because a candidate stores no decision, no second record is written and no
cross-record consistency check is needed.

`edit` is idempotent: submitting the description a node already has is a
successful no-op that moves no version and creates no commit, so retries and
sync-style callers converge instead of erroring.

### The snapshot protocol

External callers work against a version-checked protocol rather than
reimplementing capture or validation.

```rust
fn snapshot(store, vcs, id: &NodeId, context: &[String]) -> Result<WorkSnapshot>;

struct WorkSnapshot {
    schema: u32,
    node: NodeId,
    definition: DefinitionVersion,
    dependencies: Vec<ConsumedNode>,
    lineage: Vec<ConsumedNode>,
    context: Vec<ContextPin>,
    project: ProjectSnapshot,
    previous_result: Option<ResultVersion>,
}
```

`snapshot` is a pure freeze and does not require the node to be ready. Readiness
is enforced in exactly one place — `submit`, and only for conclusions that
assert success — which is what makes recording a failure always possible without
a special-case path around the protocol.

`submit` revalidates every frozen field under the mutation lock. On conflict it
records nothing and returns structured reasons:

```rust
enum SubmissionConflict {
    DefinitionChanged,
    DependenciesChanged,
    LineageChanged,
    ContextChanged { path: ProjectPath },
    ProjectChanged,
    ReadinessChanged,
    PreviousResultChanged,
}
```

Long-running workers call `snapshot` before starting and `submit` when
finished. `complete` is the one composed convenience: it performs the
snapshot/capture/submission sequence under a single lock without handing control
back to a caller between its steps.

### Publication

```rust
fn publish(vcs, candidate: &Candidate) -> Result<()>;
```

Publication writes nothing to the store. It reads the target branch, requires
that a fast-forward to the artifact is possible, and compare-and-swaps the
target from the value it just read. Whether publication succeeded is always
re-derivable from git ancestry, so retrying after a crash is safe and no journal
is needed. A target that moved forward independently fails with a plain
"cannot fast-forward" error.

### The version-control seam

Committing outputs and store changes, checking drift, and reading refs are the
only parts of Linka that need a git repository — versions and pins are computed
locally. They are routed through one trait so the rest of the library is
unit-testable with an in-memory fake: no git binary, no repository, no
configured identity.

```rust
trait Vcs {
    // store history
    fn require_clean_store(&self, path: &str) -> Result<()>;
    fn commit_store(&self, path: &str, message: &str) -> Result<()>;
    fn output_was_recorded(&self, path: &str, node: &str, commit: &str) -> Result<bool>;
    // artifacts
    fn capture(&self, paths: &[String], message: &str) -> Result<String>;
    fn capture_worktree(&self, parent: &str, message: &str) -> Result<Option<String>>;
    fn retain_output(&self, node: &str, commit: &str) -> Result<()>;
    fn drift(&self, id: &str, against: Option<&str>) -> Result<Option<String>>;
    fn files_in(&self, id: &str) -> Result<Vec<String>>;
    fn dirty_paths(&self) -> Result<Vec<String>>;
    fn commit_exists(&self, hash: &str) -> Result<bool>;
    // identity and refs
    fn head_commit(&self) -> Result<Option<String>>;
    fn linka_node(&self, commit: &str) -> Result<Option<String>>;
    fn tree_id(&self, commit: &str) -> Result<String>;
    fn file_blob(&self, path: &str) -> Result<Option<String>>;
    fn file_blob_at(&self, revision: &str, path: &str) -> Result<Option<String>>;
    fn root_commit(&self) -> Result<Option<String>>;
    fn remote_url(&self) -> Result<Option<String>>;
    fn current_branch(&self) -> Result<Option<String>>;
    fn ref_commit(&self, reference: &str) -> Result<Option<String>>;
    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool>;
    fn publish_fast_forward(&self, target: &str, expected: &str, new: &str) -> Result<bool>;
}
```

Three implementations sit behind it: the `git` subprocess one, an in-memory
fake for tests, and the offline one `check` evaluates against. The memoizing
wrapper is not a fourth — it decorates whichever of them a pass was given.

Output commits carry a `Linka-Node` trailer naming the node, and a `Linka-Input`
trailer naming the revision the work started from.

## Validation

One rule for every portable path component — node ids, project path components,
attachment namespaces and keys:

```rust
fn component(value: &str) -> Result<&str, String>;
// non-empty; no '/', '\', ':' or control characters; not "." or "..";
// not ".git" in any case; not a Windows reserved name (CON, PRN, AUX, NUL,
// COM1-9, LPT1-9); no trailing '.' or ' '
```

`.git` is forbidden without exception so graph input and output paths cannot
address repository internals. Project paths are normalized to `/` separators
and are always relative to the paired project root; absolute and
platform-prefixed paths and traversal are invalid. Working-tree reads reject
symlinks that resolve outside the project root.

Validated identifiers are newtypes over `String` whose `FromStr` runs the
validator and whose serde implementations round-trip through it, so validation
runs on deserialization as well as construction.

Two path rules that are easy to get wrong and are called out deliberately:

- **Declared outputs match dirty paths by path prefix, not string equality.**
  Git reports dirty paths per file, so a declared output directory must accept
  every file beneath it.
- **Discovery of node directories reports bad names as problems rather than
  failing.** Listing returns `(Vec<NodeId>, Vec<Problem>)`, so `check` can
  diagnose exactly the corruption it exists to find.

No code path reachable from on-disk data may panic. Hand-edited files produce
`error` states and `check` problems, never `unreachable!()` or a slicing panic.

## Integrity checking

`check` is read-only, git-free, and reports every problem write-time validation
cannot see because it entered sideways — hand edits, merges of individually
valid branches, or unsupported writers. It never stops at the first problem.

Git-free is a property of the check, not a second evaluator: the conditions it
reports include ones only the graph knows, such as two current verifications
deciding one candidate differently. It therefore runs the same evaluation pass
against an *offline* `Vcs` whose reads answer "nothing known" — no drift, no
ancestry, no refs — and whose writes refuse. That is the honest answer without
a repository, and it keeps one implementation of currency rather than a second
that could disagree with the first.

Checked per node: definition and result files parse and use supported schemas;
the result's outcome family matches the node's kind; verification results
declare no project output; dependency lists hold no duplicates or
self-references; every edge target exists; successful results carry a full pin
for every declared edge and successful evidence for every required dependency;
context and consumed pins hold no duplicates; a `verifies` node names an
existing candidate and derives from its source node; no two current
verifications decide one candidate differently; and `depends_on` contains no
cycles.

Checked per artifact, where a repository is available: recorded output commits
exist, and each completed node's output retention ref points at its recorded
artifact. Artifact repository identity is checked against the pairing only when
the store is paired; an unpaired store is a supported configuration and must not
fail its own integrity check.

`check_workbench` adds the question git can answer: whether the store's on-disk
state is fully recorded in workbench history, catching interrupted or partial
mutations that leave individually valid files behind.

## Pairing

`pairing.toml` records which project repository the store describes, keyed by
the project's *root commit* — the one hash that identifies a repository rather
than a point in its history, stable across branching, merging, and ordinary
rebases. A full-history rewrite changes it, deliberately: every recorded output
commit has just become suspect.

```toml
schema = 1
root-commit = "8a1f9c2e..."          # first-parent root of the project's HEAD
paired-at = 1719571200000
name = "splurt"                      # optional, informational only
remote = "git@host:me/splurt.git"    # optional, informational only
```

`verify_pairing` is read-only and manual; nothing calls it implicitly. The
default check is one comparison of actual root against recorded root. With
`deep`, every hash the store points at — each result's output commit and every
consumed output pin — is checked to exist, catching partial rewrites that leave
the root intact but orphan recorded outputs.

## Interfaces

The Rust library is the reference interface. The `linka` CLI exposes the same
operations to people and scripts; it is a thin dispatch over the library, with
all human formatting in a separate rendering module. An agent-facing protocol
may adapt these operations, but protocol-specific concepts do not enter the
graph model.

An orchestrator such as Orka consumes a narrow interface: read ready work,
freeze versioned input with `snapshot`, submit version-checked results and
verification conclusions with `submit`, and register candidate outputs. A review
tool such as Nota may fill verification descriptions and evidence through an
optional adapter; Linka interprets only the accepted/rejected/abandoned
conclusion, never the adapter's own schema. Executors remain one-way clients:
Linka never interprets producer namespaces.

## Module layout

| module | ~LOC | contents |
| --- | --- | --- |
| `model` | 400 | records, validated ids, derived-state types, state predicates |
| `store` | 450 | layout, record read/write, blob ids, mutation lock |
| `vcs` | 150 | the trait, the memoizing wrapper |
| `git` | 450 | the `git` subprocess implementation |
| `graph` | 400 | single-pass memoized evaluation and its projections |
| `ops` | 500 | the fact writers, `snapshot`, `complete`, `publish` |
| `check` | 250 | fsck, artifact retention, pairing verification |
| `cli` + `render` | 800 | dispatch and human formatting |

Modules export named items. No glob re-exports: the public surface is written
down, not implied.

## Testing

The in-memory `Vcs` fake models commit parentage, so `is_ancestor`,
`publish_fast_forward`, and drift are genuinely exercised — publication is
*defined* by ancestry, and a fake that answers `ancestor == descendant` makes
the normal post-merge case untestable.

Because state is a pure function of records, the load-bearing properties are
worth asserting directly: complete implies not ready; ready implies no blockers;
a definition edit makes every covering result stale; submitting against a stale
snapshot always conflicts and never writes; an unreadable record yields `error`
for that node and leaves every other node's state unchanged.

A small integration suite runs the same operations against a real temporary
workbench and the `git` binary, covering the seam the fake abstracts.

## Non-goals

- Starting or supervising agent processes.
- Docker, worktree, or network isolation policy.
- Scheduling and retry policy.
- Review comments, suggested edits, or deciding who may accept.
- Requiring Orka, Driva, or Nota for normal CLI/library use.
