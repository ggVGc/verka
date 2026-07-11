# Durable execution attempts

Status: accepted design; implementation in progress.

## Purpose

An execution attempt is a first-class durable object. A logical node may be
worked several times, and each attempt must remain independently identifiable
even when the node's convenience result is replaced. Process exit status,
timestamps, candidate branches, and mutable node results must never be used to
infer which execution produced a result.

The attempt record is written before llaundry mutates the project repository.
This makes every candidate branch and worktree explainable and recoverable.

## Storage

```text
.llaundry/attempts/<attempt-id>/
    attempt.toml
    work.jsonl
    result.toml       # optional worker-recorded outcome
    result.md         # optional worker narrative
    final.toml        # optional sealed post-session evidence
```

`attempt.toml` records the node, frozen definition, worker, force authority,
input commit/tree, candidate branch, worktree path, backend/model, creation
time, and whether workspace preparation completed.

`result.toml` belongs unambiguously to the attempt by location. It contains the
same structured result currently exposed as the node's latest-result view.
There may be at most one result per attempt.

`final.toml` records backend exit status and sealing time. Worker identity and
observed context are applied before sealing. Sealing is idempotent.

## Lifecycle

1. Authorize the node and freeze graph inputs.
2. Resolve the exact project input commit and tree.
3. Allocate the attempt ID and candidate branch.
4. Commit `attempt.toml` to the llaundry repository.
5. Create the candidate branch and linked worktree.
6. Mark preparation complete and commit that fact.
7. Run the backend with MCP authorized by only the attempt ID.
8. MCP completion/failure writes the attempt-scoped result and updates the
   node's latest-result view in the same store commit.
9. Post-session finalization attaches backend/model and observed inputs, seals
   the attempt, and creates exactly one review for any project-output result.
10. Workspace cleanup follows the sealed attempt state.

Git changes and store changes cannot form one atomic transaction. The ordering
above is therefore a compensated transaction: a crash always leaves a durable
attempt record from which missing branch/worktree preparation, finalization,
review creation, or cleanup can be diagnosed and recovered.

## Backend exit status

Backend process status is evidence, not result identity:

* output result plus exit zero: seal and create review;
* output result plus nonzero exit: seal and create review, then report backend
  failure to the caller;
* no result plus exit zero: finalization error (the backend violated its
  contract);
* no result plus nonzero exit: seal the interrupted attempt without review;
* failed or graph-only result: seal without project-content review.

An older node result can never satisfy a newer attempt because finalization
reads only `attempts/<attempt-id>/result.toml`.

## Reviews and node state

A review pins the attempt ID, candidate branch, output commit, and sealed
attempt-result version. Project output from a sealed attempt has exactly one
review regardless of backend exit status.

The node's result remains a convenience view of its latest completed attempt
for compatibility. Historical truth belongs to immutable attempt directories.
Readiness, rejection feedback, and provenance should progressively derive from
attempts and their reviews rather than overwritten node results.

## Recovery and checking

Deep checking reports:

* recorded attempt with missing candidate branch;
* candidate branch pointing at an unexpected commit;
* prepared attempt with missing worktree when one is expected;
* result without a prepared attempt;
* sealed project result without exactly one review;
* review pointing at a different attempt commit;
* final record without an attempt or result relationship.

Recovery operations may recreate safe missing workspaces, seal recorded
results, create missing reviews, or clean completed worktrees. They never
discard dirty files automatically.

## Migration

Existing node results without attempt records remain readable as legacy
results. New machine executions always use attempt-scoped storage. A later
migration may synthesize legacy attempt records where enough provenance is
available; no history is rewritten merely to adopt the new model.
