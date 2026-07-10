# Isolated worktree execution

Status: proposed feature design.

## 1. Purpose

Each `llaundry-work` invocation runs in a dedicated Git worktree created from
an exact input commit. The worker never edits the user's `project/` checkout,
two workers never share writable project state, and a successful run leaves
one independently addressable output commit.

This is execution isolation, not security isolation. The workbench geometry in
`ISOLATION.md` still keeps the llaundry store outside the worker's file-tool
grant. A worktree prevents accidental interference between runs; it does not
confine arbitrary native processes. Bubblewrap remains an optional hardening
layer.

## 2. Required properties

1. A run is anchored to one immutable project commit before the backend starts.
2. The worker's current directory is its worktree, never `project/`.
3. Runs may proceed concurrently without sharing an index, branch, or writable
   files.
4. The user's checkout, branch, index, and uncommitted changes are untouched.
5. Completion captures exactly the declared outputs in one commit whose parent
   is the run's input commit.
6. Undeclared writes cannot enter the output commit.
7. Every recorded output commit remains reachable after worktree cleanup.
8. A crash at any point is detectable and recoverable without recording a
   result that names a missing commit.
9. The store records enough information to distinguish the source state a run
   saw from the output it produced.
10. Worktree paths and temporary refs are implementation details, not identity.

## 3. Layout

The current two-repository workbench remains:

```text
workbench/                         outer llaundry repository
  .git/
  .llaundry/
  project/                         ordinary project checkout
    .git/
  .llaundry-worktrees/             ignored by the outer repository
    <run-id>/                       linked project worktree
```

The worktree administration data belongs to the project repository. For a
normal non-bare repository, Git keeps it under
`project/.git/worktrees/<generated-name>/`; the directory above contains the
working files.

`.llaundry-worktrees/` is a sibling of `project/`, not a child. This has two
useful consequences: project-wide searches from the user's checkout cannot
walk into active sessions, and a worktree cannot accidentally see another
worktree through `./**`.

The backend receives only `<workbench>/.llaundry-worktrees/<run-id>` as its
working directory and file-tool root. The MCP process receives the absolute
store path as it does today.

## 4. Identity and recorded facts

### 4.1 Node, run, and commit are distinct

A node identifies the unit of work. A run identifies one invocation attempting
that work. A commit identifies a project state. None substitutes for another.

Use an opaque random run ID, generated before any filesystem mutation. It is
safe in paths and refs and need not be meaningful to humans. The run ID appears
in the attempt event in `work.jsonl` and in diagnostic output, but does not
participate in node versions or graph semantics.

### 4.2 Result additions

A successful result with project output records:

```toml
input_commit = "0123abcd..."
input_tree = "4567efab..."
output_commit = "89abcdef..."
```

`input_commit` is the detached HEAD used to create the worktree. `input_tree`
allows the same source state to be recognized after a history rewrite; it does
not make rewritten history automatically trusted. `output_commit` has
`input_commit` as its sole parent in the first implementation.

Graph-only work also records `input_commit` and `input_tree`, even though it has
no output commit. This answers which project state the worker inspected.

The attempt header additionally records `run_id`, start time, resolved backend
and model, node definition version, and the same input commit/tree. The result
is authoritative for completed work; attempt events explain incomplete work.

No durable `in_progress` node status is added. Active runs are operational
state and may become stale after a crash.

## 5. Selecting the input commit

Input selection happens before worktree creation and must be explicit and
deterministic:

1. An explicit `--base <commit>` wins.
2. A dispatcher-created attempt uses the base frozen by the dispatch operation.
3. Otherwise `llaundry-work` resolves the paired project's current `HEAD`.

The resolved name is immediately peeled to a full commit ID and its tree ID.
The symbolic branch name is not retained as provenance.

The initial implementation refuses an unborn repository because a linked
worktree needs a commit. Existing initialization already creates an empty root
commit, so this indicates a damaged or incompletely paired workbench.

Uncommitted changes in `project/` are deliberately irrelevant: they are not in
the selected commit and are neither copied nor rejected. The launcher should
say this clearly when the user's checkout is dirty so the user does not assume
those changes were included. An explicit future `--include-working-tree`
feature would need to snapshot them into a commit; it must not silently copy
them.

Before dispatch, verify that the selected commit belongs to the paired project
repository. It need not be reachable from the user's current branch—for
example, a continuation may start from an unselected output—but it must exist.

## 6. Lifecycle

### 6.1 Prepare

The driver performs these steps in order:

1. Validate node readiness and resolve all graph/context inputs.
2. Resolve and record the input commit and tree.
3. Generate the run ID.
4. Create a durable input ref:
   `refs/llaundry/runs/<run-id>/input`.
5. Create a detached worktree:

   ```text
   git -C project worktree add --detach \
     ../.llaundry-worktrees/<run-id> <input-commit>
   ```

6. Verify the new worktree's `HEAD` and tree equal the recorded input.
7. Append and flush the attempt header to `work.jsonl`.
8. Launch the backend with the worktree as its current directory.

The input ref prevents garbage collection between resolution and completion.
The run is considered recoverable once the attempt header is durable. If an
earlier preparation step fails, cleanup may simply remove what was created.

Git serializes updates to shared repository metadata internally. llaundry also
uses a short project-repository lock around worktree add/remove/prune and ref
transactions so its own cleanup cannot race another launcher. The lock is not
held while the backend runs.

### 6.2 Execute

The backend abstraction receives an `ExecutionWorkspace` instead of the
canonical project root:

```text
run_id
node_id
path
input_commit
input_tree
store endpoint
```

All relative file paths in prompts, transcript mining, and completion are
resolved against this path. Context pinning still hashes paths from the input
state, not similarly named files in the mutable user checkout.

The worker starts on detached HEAD. It has no reason to create or switch
branches. Backend-specific user configuration must not be allowed to change
the working directory or attach unrelated MCP servers.

### 6.3 Complete

Completion is a two-phase publication: publish the project commit first, then
publish the store result. This preserves the existing rule that the store
never references an object that does not exist.

Under a per-run completion lock, the MCP completion operation:

1. Confirms it was invoked for the run's node and worktree.
2. Confirms `HEAD == input_commit`. Worker-created commits are rejected in the
   first implementation; llaundry owns the single output commit invariant.
3. Reads porcelain status including untracked files.
4. Normalizes and validates every declared output path: relative to the
   worktree, no `..`, no absolute paths, no `.git`, no out-of-tree symlink
   resolution, and no duplicate normalized paths.
5. Computes dirty paths. If any dirty path is not covered by the declared
   outputs, refuses completion and reports it. A declared directory covers its
   descendants.
6. Stages only declared outputs with pathspecs anchored at the worktree root.
7. Verifies the staged diff is non-empty when outputs were declared, and that
   its paths exactly match dirty declared paths.
8. Creates one commit with parent `input_commit`, including trailers:

   ```text
   Llaundry-Node: <node-id>
   Llaundry-Run: <run-id>
   Llaundry-Input: <input-commit>
   ```

9. Atomically creates
   `refs/llaundry/outputs/<node-id>` at the new commit, requiring that the ref
   did not previously exist for a first completion. Rework policy may replace
   it only with an explicit expected-old value.
10. Writes `result.toml`/`result.md`, including input commit/tree, output
    commit, dependency pins, context pins, and worker metadata; then commits
    the store change.

For graph-only completion, steps 6–9 are omitted, but the worktree must be
clean and the result still records its input commit/tree.

The output ref is a reachability index, not the source of truth. Result records
define provenance. A check/repair command can recreate missing refs from
results and flag refs that disagree.

### 6.4 Finish and cleanup

After the backend exits, the driver commits the transcript tail as today. It
then handles the worktree according to state:

* **completed and clean:** remove the worktree, delete the run input ref;
* **failed before making changes:** remove it, delete the input ref;
* **paused on a question:** retain it by default so continuation can use the
  exact filesystem state;
* **backend failure with changes or completion refusal:** retain it for
  inspection and recovery;
* **explicit discard:** remove it only after the user requests discard.

Retained worktrees are reported with their absolute path and run ID. Automatic
cleanup must never discard uncommitted files.

`git worktree remove` is used without `--force` normally. Forced removal is
reserved for explicit discard after the driver has independently checked and
reported dirtiness.

## 7. Pause, resume, and retry

Transcript replay alone cannot restore uncommitted edits. Therefore a paused
run retains its worktree and input ref. Continuing the node first tries to
resume that run after verifying:

* the worktree is registered with the paired project repository;
* its `HEAD` is still the recorded input commit;
* its path has not escaped the configured worktree root;
* the node remains open and has no newer run selected for continuation.

If the worktree is gone but was clean, replaying the log in a newly created
worktree at the same input is valid. If the log records writes and the
worktree is gone, the driver warns that filesystem state was lost and requires
an explicit restart; it must not pretend this is a continuation.

A retry after a genuine failed attempt creates a new run ID and normally
resolves a fresh base. `--retry-run <id>` instead uses the failed run's exact
input commit. Retrying never reuses a dirty worktree implicitly.

## 8. Crash consistency and recovery

The ordered publication protocol has intentional intermediate states:

| Observed state | Meaning | Recovery |
|---|---|---|
| worktree/ref, no attempt header | preparation crashed | inspect, then remove if clean |
| attempt header, worktree present, no result | active, paused, or crashed | inspect PID/lease; resume or discard |
| output commit/ref, no result | project publication succeeded; store publication crashed | reconstruct and finish result, or preserve for inspection |
| result names commit, ref missing | ref index damaged | recreate ref after verifying commit |
| result names missing commit | invariant violation/history damage | report loudly; never silently rerun |
| result committed, worktree present | cleanup crashed | remove if clean |

Operational run metadata should live outside node definitions, either as
attempt events plus Git/worktree discovery or in a dedicated
`.llaundry/runs/<run-id>.toml` journal excluded from graph semantics. If a run
journal is introduced, every transition is an atomic rewrite followed by a
store commit; it is not used to derive node status.

Provide an idempotent command such as `llaundry worktree recover` that:

1. lists registered worktrees, run refs, attempt headers, output refs, and
   results;
2. classifies them using the table above;
3. performs safe repairs automatically;
4. requires explicit confirmation before discarding dirty state.

Age alone never proves a run is abandoned. A lease containing PID, host, and
last heartbeat can identify likely stale local runs, but removal still follows
the dirtiness rule.

## 9. Concurrency rules

Different nodes may run concurrently without restriction when their bases are
already frozen. Repeated execution of the same node is rejected by default
while it has a live or retained run. Parallel alternatives must use distinct
child nodes, as described in `PARALLEL_VARIANTS.md`; this preserves the
one-node/one-result invariant.

Completion uses compare-and-swap checks for both graph and Git state:

* the node definition version must still equal the version captured at launch;
* dependency/result/context pins are checked against the captured inputs;
* the node must not already have a result written by another run;
* creation/update of the output ref uses an expected old object ID.

If any check fails, the commit and worktree remain available, but publication
stops. The output may be adopted by an explicit recovery/reconciliation
operation after review; it is never silently attached to changed work.

## 10. Changes to the current implementation

### 10.1 Execution workspace

`llaundry-work` currently sets `Session.project_root` to `store.project_root()`.
Replace this with a prepared workspace object and run the backend there.
Transcript path normalization must use the workspace root while context blob
lookup uses the recorded input commit/tree.

### 10.2 VCS seam

The current `Vcs` combines read operations on the canonical checkout with
`capture`, which commits there. Split the concepts:

```text
ProjectRepo
  resolve_commit(rev)
  tree(commit)
  create_worktree(run, commit)
  remove_worktree(run)
  update_ref(ref, new, expected_old)
  commit_exists(commit)

ExecutionTree
  head()
  dirty_paths()
  capture(paths, message, trailers)
  blob_at_input(path)
```

Store commits remain a separate workbench-repository operation. This makes it
impossible to accidentally ask the canonical checkout to capture an isolated
run's files.

### 10.3 Completion API

`complete_node` must be run-scoped. The MCP server should receive an unforgeable
run token or a private run descriptor and bind completion to its worktree.
Accepting only a node ID would let a session complete the node using the wrong
checkout.

Human CLI completion can retain the current canonical-checkout mode as a
separate explicit path, or later be migrated to preparation of a worktree. It
must not be confused with backend completion.

### 10.4 Drift

Current drift compares an output commit with the mutable project checkout.
Once outputs are independent branches, drift needs a comparison target:

* output integrity: verify the commit and recorded ref still exist;
* applicability to a projection: compare/replay against that projection;
* worktree dirtiness: inspect only the active execution tree.

An unselected output is not stale merely because `project/` does not contain
its files.

## 11. Command surface

Names are illustrative:

```text
llaundry-work <node> [--base <commit>] [--keep-worktree]
llaundry worktree list
llaundry worktree inspect <run-id>
llaundry worktree resume <run-id>
llaundry worktree discard <run-id>
llaundry worktree recover [--repair]
llaundry refs check [--repair]
```

`--dry-run` shows the resolved input commit and intended worktree path but
creates neither refs nor directories.

## 12. Security and filesystem details

* Reject a configured worktree root that resolves inside `project/`, the store,
  or another active worktree.
* Preflight committed symlinks whose resolved targets escape the worktree; this
  complements the backend's path-scoped permission enforcement.
* Never pass user-controlled IDs directly as ref or directory components.
* Disable repository-local hooks for llaundry-owned commits, or invoke commits
  with a controlled hooks directory. Hooks can mutate unrelated files or
  produce extra commits and violate the lifecycle.
* Use `--literal-pathspecs` or equivalent path validation so output names cannot
  be interpreted as Git pathspec magic.
* Do not share a branch between worktrees. Detached HEAD plus private refs avoids
  Git's branch checkout restrictions and accidental branch movement.
* Submodules require a separate design. Initially reject repositories with
  active submodules or declare that they are read-only inputs; linked worktree
  isolation does not automatically provide isolated nested repositories.

## 13. Minimal implementation stages

### Stage 1: isolated single runs

* create detached worktree from resolved `HEAD`;
* run backend inside it;
* commit declared outputs there;
* record input commit/tree and output commit;
* create durable output refs;
* remove clean completed worktrees.

### Stage 2: recovery

* retain dirty/failed worktrees;
* run IDs and input refs;
* list, inspect, discard, and idempotent recovery commands;
* compare-and-swap publication checks.

### Stage 3: continuation and parallel dispatch

* resume paused worktrees;
* freeze shared bases for explicit fan-out;
* concurrent worktree lifecycle tests;
* integration with variant lineage and selection.

### Stage 4: hardening

* controlled Git hooks and submodule policy;
* escaping-symlink preflight;
* optional bubblewrap execution;
* ref/store consistency checking and repair.

## 14. Acceptance scenarios

The feature is complete when automated tests demonstrate at least:

1. A worker changes a file while the user's dirty checkout remains byte-for-byte
   and index-for-index unchanged.
2. Two nodes starting at the same commit produce different commits concurrently.
3. An undeclared file causes completion to fail and remains recoverable in the
   run worktree.
4. A successful output commit remains readable after its worktree is removed
   and `git gc` runs.
5. A crash after output-ref creation but before result writing is classified and
   recoverable.
6. A definition edit during execution prevents publication without deleting
   the produced commit.
7. Paused work resumes with both transcript and uncommitted filesystem state.
8. Cleanup never force-removes a dirty worktree automatically.
9. No run moves the user's checked-out branch or changes its index.
10. Context pins are computed from the run's input/worktree state rather than
    the current contents of `project/`.

