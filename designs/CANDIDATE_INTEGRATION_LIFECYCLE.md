# Candidate integration lifecycle: accept, publish, and stale bases

This note explains what "accepting" and "publishing" a candidate mean in Orka,
what happens when a publish fails, and how to recover when the target branch has
moved out from under an accepted candidate.

## Attempts vs. candidates

Two distinct record types are involved:

- An **attempt** (`orka/src/attempt.rs`) is Orka's durable, append-only record of
  one execution run.
- A **candidate** (`linka/src/candidate.rs`) is Linka's proposed project output,
  attached to a node result. **Accept and publish operate on candidates, not on
  attempts directly.** Orka's `Candidates` wrapper (`orka/src/candidate.rs`) lets
  you name a candidate by its Linka id or by the Orka attempt id that produced it.

A candidate binds three things immutably: the node, a specific `result_version`,
and a specific `artifact` commit id. The record is immutable except for its
`state` field — there is no API to repoint `artifact.id` at a new commit.

## Accept vs. publish

The lifecycle is `Pending -> Accepted -> Published` (`IntegrationStatus`,
`linka/src/model.rs:416`). The two operations are deliberately split into
*decision* and *integration*:

- **Accept** (`orka accept`; `orka/src/candidate.rs:93` -> `linka/src/candidate/operations.rs:68`)
  records a durable decision. It moves state `Pending -> Accepted`, capturing the
  author, notes, and a snapshot of where the target branch currently points
  (`target_previous`). **It does not touch the target branch.** It is pure intent.
- **Publish** (`orka publish`; `orka/src/candidate.rs:115` -> `operations.rs:130`)
  performs the physical integration: a **fast-forward** of the target branch to the
  accepted artifact's commit (`publish_fast_forward`, `linka/src/git.rs:247`). It
  writes **no state to Linka** — "published" is derived from Git history (the
  artifact being an ancestor of the target). Publish requires the candidate to
  already be `Accepted`.

In one line: accept is a recorded intent (no branch change); publish is the actual
fast-forward merge into the target branch.

Naming caveat: "accept" also appears at the attempt-seal layer
(`SealedState::Submitted` = "the graph accepted the result", `attempt.rs:86`).
That is Linka accepting a *result submission* at seal time — a different thing
from the candidate `accept` operation.

## What else you can do with an accepted candidate

The state machine is intentionally restrictive once accepted:

- **Publish it** — the intended next step.
- **Accept again** — idempotent no-op (`operations.rs:78`).
- **Inspect it** — `orka candidate <id>`, `orka candidates`, `orka audit`, or the
  patch (`git diff input..head`).
- **Review it** — the Nota review subsystem (`orka review start/resume/finish/abandon`).

Not available once accepted:

- **Reject** — only accepts a `Pending` candidate; bails otherwise
  (`operations.rs:98`). No reject-after-accept.
- **Unaccept / revert / unpublish / withdraw / re-run / edit** — none exist.
  `Accepted` has no exit path except deriving `Published` from Git.

To supersede accepted work you don't mutate the candidate — you edit the *node
definition*, which reopens the done node (`linka/src/model.rs:513`;
`editing_a_done_node_reopens_it`, `linka/src/ops.rs:2127`) and produces a **new**
result and candidate. The original accepted candidate is left behind.

## What happens if a publish fails

Publish writes no Linka state and only mutates the Git branch pointer atomically,
so a failure is safe and retryable. The candidate stays `Accepted`; re-run
`orka publish` once the cause is resolved. Publish is idempotent (returns `Ok(())`
early if the artifact is already an ancestor of the target, `operations.rs:138`).

Failure modes:

1. **"cannot fast-forward its target branch"** (`operations.rs:147`) —
   `publish_fast_forward` returned `false`: the target no longer points at
   `target_previous` (`git.rs:253`), the artifact is not a descendant
   (`git.rs:260`), or the checked-out HEAD moved off `target_previous`
   (`git.rs:269`).
2. **"project checkout is dirty; ..."** (`git.rs:267`) — the target is the checked-out
   branch and the working tree is dirty. Commit or stash, then retry.
3. **"candidate is no longer the current ... candidate for node"**
   (`operations.rs:164`) — the source result moved or a newer candidate superseded
   this one.

There is no half-published state: the checked-out path uses `git merge --ff-only`
(`git.rs:272`) and the ref-update path uses a compare-and-swap
`git update-ref target candidate expected_previous` (`git.rs:276`). Either the
branch advanced or it did not. Even a crash mid-publish leaves a clean, retryable
`Accepted` state ("Git history is the publication record ... needs no Linka
journal", `operations.rs:128`).

## Recovering when the fast-forward preconditions no longer hold

Split by cause:

**Recoverable in place — fix and re-run `orka publish`:**

- Dirty checkout (`git.rs:267`): commit or stash the working tree, then publish.
- Target already contains the artifact: publish is a no-op; already published.

**Divergence — target moved to a commit that does NOT contain the artifact:**

The accepted candidate can no longer fast-forward and cannot be salvaged in place.
`integration()` does not even return `Accepted` — it **bails**:
`"target moved from X to Y without containing Z"` (`candidate.rs:88`). There is no
unaccept / rebase-candidate / re-target operation.

Expected action: let the node's staleness machinery drive re-work. The moved base
makes the node's output drift (`StalenessReason::OutputDrifted`, `ops.rs:718`) and
`Currency::Stale`. Reopen the node (edit its definition), which produces a new
attempt -> result -> candidate built on the *current* target. Accept and publish
that fresh candidate. The old one is superseded.

| Precondition failure | Expected action |
|---|---|
| Dirty checkout (`git.rs:267`) | Clean the working tree, re-run `orka publish` |
| Target already contains the artifact | Nothing — already published (idempotent) |
| Target diverged (`candidate.rs:88`) | Reopen the node to regenerate a candidate on the new base, then accept + publish |

## "I want to rebase the candidate, not rebuild it"

There is no first-class rebase-candidate operation, and you cannot get there by
mutating the existing candidate — its `artifact`/`result` binding is immutable and
both publish-time guards enforce it:

- `require_current` bails unless the node's stored result still matches
  `candidate.result` and `result.output == candidate.artifact`
  (`operations.rs:161`).
- `publish_fast_forward` only succeeds if `candidate.artifact.id` descends from
  `target_previous` (`git.rs:256`).

If you rebase the changes yourself in Git you produce a *different* commit, which
is not the candidate's artifact — `orka publish` won't recognize it and
`integration()` keeps reporting divergence. A manual `git update-ref` moves the
target outside Orka's knowledge, drifting the node's output and cascading `Stale`
downstream. That fights the model.

The supported way to reuse the work (not rebuild it): run a **new attempt** on the
reopened node whose workspace starts from the current target and re-applies the
prior work. The old artifact commit still exists in Git, so the attempt can
cherry-pick/rebase it onto the moved base and resolve conflicts — ordinary Git work
in the workspace. The attempt submits the rebased tree as the node's **new result**
(`submit_result`, `ops.rs:854`; Orka side `submit_candidate_success`,
`orka/src/linka_work.rs:340`). That new result produces a new candidate whose
`target_previous` is the current target; accept it, then publish (the fast-forward
now succeeds).

So you keep the *work* (the diff, the resolved conflicts) but not the *candidate
record*. This is the same situation the seal layer models as
`StaleAtSubmit { conflicts }` (`attempt.rs:93`): the graph moved between snapshot
and submit, which is "an answer, not an operational error" (`linka_work.rs:34`).
Forcing the rebase through a fresh result submission preserves the core invariant —
whatever lands on the target was validated against the base it will actually sit on.
A rebased-in-place candidate would have been reviewed/accepted against the old base
but published onto a new one, which is exactly the gap the fast-forward-only rule
closes.

## Key files

- `orka/src/attempt.rs` — attempt record, phases, seal states.
- `orka/src/candidate.rs` — Orka's accept/reject/publish wrappers.
- `linka/src/candidate.rs` — `CandidateState`, `integration()` derivation.
- `linka/src/candidate/operations.rs` — accept/reject/publish logic.
- `linka/src/git.rs` — `publish_fast_forward`.
- `linka/src/model.rs` — `IntegrationStatus`.
- `linka/src/ops.rs` — staleness, result submission, node reopening.
- `orka/src/linka_work.rs` — settle/recovery, result submission from attempts.
- `orka/src/main.rs` — CLI subcommands.
