# Orka design

## Purpose

Orka orchestrates isolated agent attempts for work in a Linka store. It uses
Linka to discover, freeze, and record work, and Driva to execute agent commands
in isolation. Orka owns coordination and durable attempts; it does not implement
container execution or human review.

Orka is specifically a Linka orchestrator. It depends on Linka's public library
API and value types directly — it does not maintain a backend-neutral graph port
or a duplicate graph model, and it does not pretend other graph backends are
supported. The dependency direction is one-way: Orka depends on Linka; Linka
never depends on Orka.

## Ownership

- Linka owns graph definitions, readiness, staleness, work snapshots, result
  validation, candidates, accept/reject decisions, Git-derived publication,
  graph mutations, and project/output provenance.
- Orka owns agent selection policy, execution policy, prompts, durable
  attempts, transcripts, outcome interpretation, candidate presentation,
  attempt recovery, workspace cleanup, and coordination of Git-native Nota
  reviews with Linka verification nodes.
- Orka calls Linka's public operations but never reads or mutates Linka's
  on-disk representation directly.
- Linka stores only namespaced producer evidence about Orka (namespace `orka`);
  it never interprets attempts, agents, executors, or recovery state. Linka's
  generic node attachments also retain opaque Orka evidence for produced
  outputs without adding Orka semantics to Linka.
- `.linka/` and `.orka/` are separately owned stores in the workbench.
- Nota depends only on Git. Orka resolves Linka candidates to Git artifacts and
  owns the binding between a Nota branch and a Linka verification node.

## Linka protocol

Orka uses one documented Linka protocol:

- `linka::WorkSnapshot` is the authoritative frozen work input. It freezes node
  identity, definition version, dependency and lineage pins with outcomes,
  explicit context pins, the project repository/revision/tree, and the previous
  result version. `ops::snapshot_work` produces it; Orka persists it verbatim.
- `ops::capture_submission` consumes a caller's frozen snapshot, captures the
  declared outputs in the supplied `Vcs` execution context, and submits a
  version-checked result (success with or without outputs, or failure). It
  revalidates every frozen field and, on a conflict, records nothing and
  retains no output ref — stale work never silently completes. Conflicts come
  back as `SubmissionError::Conflict(Vec<SubmissionConflict>)`; other errors are
  reserved for evaluation, storage, git, or invariant failures.
- `CandidateStore::register` attaches a successful project output to its exact
  node result, immutable branch, frozen input, target branch, and opaque Orka
  attempt identity. Linka derives pending/accepted/published/rejected
  integration state and excludes pending work from redispatch.

## Boundaries

Only two dependencies are genuinely replaceable, so only these stay narrow
Orka-owned traits:

```rust
trait IsolatedExecutor {
    // Run a command with a concrete filesystem and network capability grant.
}

trait WorkspaceManager {
    // Prepare and clean isolated per-attempt working trees.
}
```

Production adapters use Driva and git worktrees; tests substitute fakes for both
(the Linka store is always real). Everything else — selection, snapshotting, and
submission — goes through `linka_work::LinkaWork`, a concrete integration with
Linka, not a backend-neutral port.

## Attempt lifecycle

1. Select Linka-ready, machine-assignable work (`ops::ready_nodes(..,
   Some(Author::Machine))`). Orka chooses among Linka-ready results; it does not
   derive readiness.
2. Ask Linka to validate and snapshot the node, and gather the prompt prose, as
   one durable `AttemptInput` (Linka's `WorkSnapshot` plus the description and
   related-work prose). Record it before any side effect.
3. Prepare an isolated worktree at `snapshot.project.revision`.
4. Choose the exact mounts, network policy, agent command, and context, then
   record the Driva execution request before starting the command.
5. Capture raw agent events, separate diagnostics, a readable transcript,
   harness-observed project file reads, and exit evidence. Provider event
   decoding and filesystem observation remain in Orka; Driva transports
   uninterpreted streams.
6. Interpret the agent's declared outcome (Orka's `AgentOutcome`), then submit
   through Linka against the exact persisted snapshot, attaching the executor
   report as `orka`-namespaced producer evidence. The agent must commit all its
   work: a declared success that leaves the worktree dirty captures nothing and
   is rejected as a contract violation. The captured output is the diff between
   the frozen input commit and the committed worktree; no output paths are
   declared.
7. After an accepted result, record content pins for observed reads against
   that exact result version. Recovery repeats this idempotently if a crash
   occurs between result acceptance and attempt sealing.
8. For a successful project output, idempotently register a Linka candidate
   using the Orka attempt as opaque external identity.
9. Seal accepted success, accepted failure, or a submission conflict
   (stale-at-submit). Operational failures stay unsealed and recoverable.

## Agent authority

Orka turns graph context into a concrete capability grant. An agent sees only
the files, mounts, and network access needed for its attempt. Authorization is
enforced by adapters and scoped tools, not merely described in a prompt. Backend
and model evidence come from the harness (the executor report), never from agent
claims. Only trusted Orka code translates an `AgentOutcome` into a Linka
mutation; agent-written TOML is never deserialized into a Linka submission.

## Durability and recovery

An attempt is written before external side effects, one file per step, so its
phase is derived from which files exist. Recovery classifies each attempt by its
files and finishes the idempotent remainder:

- Never invent an outcome without exit evidence: a changed pre-evidence
  attempt seals interrupted. An entirely unchanged executor failure is rolled
  back, including its empty attempt record and candidate branch.
- Resubmit executed-but-unsealed attempts against the persisted snapshot;
  Linka's version check makes re-submission safe and non-duplicating.
- Never discard a dirty workspace; clean only sealed attempts or attempts that
  cannot have a result.

Ordinary cleanup of a sealed attempt does not remove its candidate branch.
Orka deliberately retains `orka/attempts/<attempt-id>` branches for accepted,
failed, stale, and otherwise sealed attempts so their candidate state remains
available for inspection, recovery, or later review. These branches are part
of the attempt's evidence and are not garbage-collection candidates. The only
automatic rollback is a pre-evidence executor error whose worktree and branch
still exactly match the frozen input; there is no work to preserve in that
case. Any broader deletion requires an explicit pruning operation with a
visible retention policy.

## Candidate integration

Linka is authoritative for candidates and their integration. Orka lists and
renders Linka records, resolves its own attempt ids through Linka's opaque
external identity, and delegates accept, reject, and publish to Linka. It
neither duplicates candidate state in `.orka/` nor moves the target branch
itself.

Only Linka-accepted successful outputs become candidates. A stale-at-submit
branch remains durable Orka attempt evidence but cannot enter Linka's
acceptance protocol because Linka recorded no result for it.

## Candidate verification

`orka review start <candidate>` creates one active ordinary Linka verification
node per candidate, freezes its `WorkSnapshot`, and starts a Nota branch at the
candidate's exact Git artifact. Repeating the command while that review is
active resumes the existing binding. Orka records the immutable binding under
`.orka/reviews/<verification>/review.toml` before creating the branch. Nota
receives only a repository, revision, and branch; it has no Linka dependency.
Once that verification is finished or abandoned, another review can create a
new verification for the same candidate.

Reviewers use Nota directly for notes and ordinary Git commits for suggested
edits. `orka review finish` loads and validates that Git evidence by branch and
submits a graph-only verification result against the frozen Linka snapshot with
`orka.nota` producer evidence.
`orka review list` derives active reviews from bindings whose verification has
no result. `orka review abandon` submits a graph-only failed result with
explicit abandonment evidence and leaves both the binding and Nota branch
intact; a later start for the candidate creates a new verification.
The verdict is evidence, not acceptance policy: accepting, rejecting, and
publishing the candidate remain explicit operations. If graph inputs moved
during review, Linka rejects submission and the Nota branch remains intact.

Orka manages an optional canonical checkout for each review below
`.orka/review-worktrees/<verification>/`. Entering a review creates or reuses
that checkout only after verifying its registered branch. Cleanup refuses
uncommitted changes and never deletes the Nota branch, which remains the
durable review record. Nota itself stays independent of worktree lifecycle
policy.

## Producer evidence

Every agent-attempt result carries `linka::ProducerEvidence` in the stable
`orka` namespace: the attempt id, executor-observed backend, start/finish
timestamps, exit code, and access-tracking completeness. Coordinated review
results use `orka.nota` with the candidate, verification, and branch plus
either the marker, review head, and verdict or an explicit abandoned status.
For a successful agent outcome, Orka additionally stores the exact attempt
input, prompt, execution request, transcript, harness evidence, and declared
outcome as opaque Linka node attachments before submitting the result. Mutable
filesystem paths and recovery state stay in `.orka/`. Linka preserves this
evidence verbatim and never interprets it.

On Linux, Orka watches the attempt's unique host-side worktree with inotify
during execution and writes `accesses.v1.jsonl`. Reads are project-relative,
deduplicated, and exclude `.git`; paths covered by node outputs are excluded by
Linka when observations are recorded. Tracking failures and queue overflow are
recorded as incomplete rather than presented as an empty complete read set.

## Non-goals

- Implementing isolation mechanics or process stdio (Driva).
- Owning node-graph semantics or storage (Linka).
- A generic, backend-neutral graph interface. Orka orchestrates Linka.
- Implementing review comments or suggested edits (Nota owns their Git
  representation).
- Treating a review verdict as authorization to accept or publish.

## Configuration

Orka configuration (`orka.toml`) selects an Orka-owned coding-agent profile or
literal command, additional mounts, network policy, and the Driva isolation
backend. Orka resolves the complete agent invocation and workspace protocol;
Driva contributes execution and isolation mechanics, not templates. None of
these concerns belong in Linka.
