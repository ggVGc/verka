# Linka design

## Purpose

Linka is a standalone, git-versioned graph of work nodes. It records what work
means, how work items relate, and what results were produced. It does not run
agents, manage containers, orchestrate attempts, or interpret reviews.

## Model

A node has an immutable identity and versioned definition. Directed edges
express dependencies, lineage, and explicit context. A result pins the exact
definition and inputs it covered and may declare output artifacts.

Graph state is derived, never stored as one `status` value. It has four
independent dimensions:

- **Recorded outcome** is `open`, `succeeded`, or `failed`.
- **Currency** is `current` or `stale` against the exact definition, inputs,
  context, and artifact the result recorded.
- **Integration** is `not-required`, `pending`, `accepted`, `published`, or
  `rejected`. Direct results need no integration. Candidate results remain
  current on their immutable branches while awaiting a decision; absence from
  the target branch is not staleness.
- **Workability** is `complete`, `ready`, `awaiting-integration`, or `blocked`.

These dimensions obey the following rules:

1. A node is **complete** exactly when it has a successful, current result whose
   integration is either not required or published.
2. A node is **awaiting integration** while its current successful candidate is
   pending acceptance or accepted but unpublished. It is not redispatched.
3. A node is **ready** when it is neither complete nor awaiting integration and
   all current `depends_on` targets are complete. Rejecting the current
   candidate returns the node to ready.
4. Other valid nodes are **blocked** by incomplete `depends_on` targets.
5. `derived_from` records lineage and provenance but does not gate readiness.

Candidates are first-class records attached to an exact node result version
and immutable output artifact. They are not ordinary work nodes, so rejected
alternatives do not become dependencies or poison graph settlement. A
candidate pins its artifact and intended target branch and may carry a display
branch and opaque producer identity. Linka never interprets producer
namespaces; an executor such as Orka remains a one-way client.

| Outcome | Currency | Integration | Dependencies | Workability |
| --- | --- | --- | --- | --- |
| open | current | not-required | complete | ready |
| failed | current or stale | not-required | complete | ready |
| succeeded | current | not-required or published | complete | complete |
| succeeded | current | pending or accepted | complete | awaiting-integration |
| succeeded | current | rejected | complete | ready |
| succeeded | stale | any | complete | ready |
| any | any | any | incomplete | blocked |
| unreadable | unknown | unknown | unknown | error |

Missing facts with defined semantics are not corruption: an absent result is
`open`, a context path proven absent is stale, and an artifact proven absent is
stale. In contrast, failures to read or parse definitions or results, failures
to inspect context, and artifact-backend failures are errors. Queries must
return those errors rather than converting them to `open`, `ready`, `blocked`,
or `stale`.

Review discussion and authorization policy belong to other applications.
Linka records exact accept/reject decisions and owns safe publication after an
authorized caller requests it.

## Storage

The default store is `.linka/` in a Git workbench. Node definitions, results,
and logs use inspectable TOML and Markdown. Mutations are committed so Git
provides history, integrity, blame, and distribution; Linka provides the graph
semantics Git cannot express.

Every store mutation follows one transaction boundary: Linka acquires the
workbench-wide mutation lock, refuses to proceed unless the tracked `.linka/`
store is clean, performs the complete action, commits it as one Git commit,
verifies that the store is clean again, and releases the lock. The lock itself
lives under the workbench repository's `.git/` directory and uses an OS file
lock, so it never enters a store commit and is released if a process exits.
Failed writes or commits may leave evidence in the working tree, but that dirty
state blocks every later mutation until it is explicitly resolved.

Short-lived completion holds the store mutation lock from its clean-store
precondition through result commit. It commits declared outputs in the separate
project repository before recording the result in the store. There is
deliberately no procedural submission journal. If recording the result fails,
Linka reports the created output commit; if the process is interrupted, the
library refuses a later short-lived completion from a project `HEAD` carrying a
`Linka-Node` trailer that has never appeared in committed store history.
Previously recorded historical outputs remain valid evidence. Read-only
inspection remains available while resolving such an interrupted completion.

Definitions and results are never overwritten as hidden mutable state. Stored
facts are minimal; readiness, blockers, dependents, provenance, and staleness
are computed from them.

Each candidate lives in one `candidates/<candidate-id>/candidate.toml` record.
The record contains its identity, source result, artifact, display branch,
target, and pending/accepted/rejected state; Git history provides the decision
audit trail. The artifact commit is authoritative—the producer's branch is
informational and may be moved or removed without changing candidate validity.
Acceptance pins the target branch's previous commit. Publication
compare-and-swap fast-forwards the target; whether it succeeded is derived from
Git ancestry. Retrying is safe after a crash, and a target that moved without
containing the candidate is reported as an integrity error.

Node identifiers are single portable path components. Project paths are
normalized to `/` separators and are always relative to the paired project
root. Empty components, absolute and platform-prefixed paths, traversal,
control characters, and any `.git` component are invalid. `.git` is forbidden
without exception so graph input and output paths cannot address repository
internals. Working-tree reads must also reject symlinks that resolve outside
the project root.

## Interfaces

The Rust library is the reference interface. The `linka` CLI exposes the same
operations to people and scripts. An agent-facing protocol may adapt those
operations, but protocol-specific concepts do not enter the graph model.

Orka consumes a narrow graph interface for reading ready work, freezing
versioned input, submitting version-checked results, and registering candidate
outputs. Nota may use an
optional adapter to persist or link review records, but Linka never interprets
their schema.

Long-running workers must call `snapshot_work` before starting and
`submit_result` when finished. Submission compares the frozen definition,
dependency, lineage, context, readiness, and previous-result versions under the
store mutation lock. `complete` is only a short-lived convenience that performs
that snapshot/capture/submission sequence without handing control back to a
caller between its steps.

## Non-goals

- Starting or supervising agent processes.
- Docker, worktree, or network isolation policy.
- Scheduling and retry policy.
- Review comments, suggested edits, or deciding who may accept.
- Requiring Orka, Driva, or Nota for normal CLI/library use.
