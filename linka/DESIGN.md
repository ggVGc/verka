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

- **Recorded outcome** is `open`, `succeeded`, or `failed` for ordinary work,
  and `open`, `accepted`, `rejected`, or `abandoned` for verification.
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

A verification is a review node with the typed fact `verifies`, which names the
exact candidate under review. Its `derived_from` lineage must include that
candidate's source node, so its result pins the exact source result and artifact.
Unlike ordinary work, a verification cannot be completed or failed. Its result
is `accepted`, `rejected`, or `abandoned`. All three are terminal for that
verification, while only `accepted` satisfies a downstream `depends_on` edge.
`Rejected` means the review rejected the candidate; `abandoned` means no
candidate decision was reached.

Submitting `accepted` or `rejected` atomically records the matching candidate
decision in the same Linka commit. Linka checks that the verification names the
candidate and pinned that candidate's exact result and artifact. An `abandoned`
result records no candidate decision. Several verification nodes may refer to
the same candidate, but a decided candidate records the exact verification that
decided it. Review tooling may attach opaque producer evidence, while Linka
owns and interprets the review conclusion itself.

Applications may also associate opaque attachments with a node under a
namespaced key. Linka commits the exact bytes and basic content metadata but
never interprets them or includes them in definition, result, readiness, or
staleness semantics. Attachments are immutable: recording identical data again
is idempotent, while changing an existing namespace/key is refused. This lets a
producer retain durable evidence without adding producer-specific concepts to
the graph model. Applications may record several attachments atomically in one
Linka mutation.

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
| any | any | any | incomplete | blocked |
| unreadable | unknown | unknown | unknown | error |

Missing facts with defined semantics are not corruption: an absent result is
`open`, a context path proven absent is stale, and an artifact proven absent is
stale. In contrast, failures to read or parse definitions or results, failures
to inspect context, and artifact-backend failures are errors. Queries must
return those errors rather than converting them to `open`, `ready`, `blocked`,
or `stale`.

Review discussion and reviewer authorization policy belong to other
applications. Linka enforces that candidate decisions agree with current review
results and owns safe publication after an authorized caller requests it.

## Storage

The default store is `.linka/` in a Git workbench. Node definitions, results,
attachments, and logs use inspectable metadata and payload files. Mutations are
committed so Git provides history, integrity, blame, and distribution; Linka
provides the graph semantics Git cannot express.

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

The Rust library is the reference interface. The `linka` CLI — a separate
crate, `linka-cli` — exposes the same operations to people and scripts. An agent-facing protocol may adapt those
operations, but protocol-specific concepts do not enter the graph model.

Orka consumes a narrow graph interface for reading ready work, freezing
versioned input, and submitting version-checked work and verification results.
An output-producing submission may register its candidate, attachments, and
observed context in that same store mutation; it never needs a second decision
write after a verification submission succeeds. Nota may use an optional adapter to fill
verification descriptions and evidence; Linka interprets only the
accepted/rejected/abandoned conclusion, never Nota's own schema.

The library has one injectable `Vcs` seam. Its methods are grouped in the API
documentation by store history, artifacts, context identity, repository
identity, and named references, but applications and tests provide one backend
for an operation. `GitVcs` is the production backend and the in-memory fake is
used by graph tests; this is an abstraction boundary, not a promise of a
multi-backend capability hierarchy.

Definition and result loaders parse a record and calculate its version from the
same bytes. An absent pair of result files is an absent result; a partial pair
or malformed record is an error. This makes a single record read internally
self-consistent, but it is not a filesystem-wide snapshot: concurrent external
writers may still change another record between reads. Mutation revalidation
therefore always creates a fresh view under the store lock.

Long-running workers must call `snapshot_work` before starting and
`submit_result` when finished. Submission compares the frozen definition,
dependency, lineage, context, readiness, and previous-result versions under the
store mutation lock. `complete` is only a short-lived convenience that performs
that snapshot/capture/submission sequence without handing control back to a
caller between its steps.

Submission entry points intentionally retain these policy differences:

| entry point | input snapshot | readiness | project-tree policy | output/retention |
|---|---|---|---|---|
| `complete` | captured while locked | required | clean except declared outputs | captures and retains after acceptance |
| `respond` | captured while locked | required | dirty tree allowed | no output |
| `fail` | current pins while locked | may record blocked work | dirty tree allowed | no output |
| checked work submission | caller-frozen | revalidated | caller owns execution tree | supplied artifact retained after acceptance |
| verification submission | caller-frozen | revalidated | no project output | atomically records decision |
| captured execution submission | caller-frozen | revalidated | execution tree is checked before capture | capture precedes validation; retention follows acceptance |

Attachments, an optional candidate, and discovered context are prepared as one
checked submission and committed with its result. Discovered paths describe
what execution actually read; they do not retroactively become declared inputs
of the frozen snapshot. They are identified at the frozen revision and checked
under the mutation lock. Candidate publication and artifact capture/import are
separate project-repository operations.

Historical `observations/*.toml` files remain a supported read format. They are
not folded into old `result.toml` files because doing so would change result
versions and invalidate downstream pins. The narrow late-observation writer is
retained for result-only and failed legacy producers; new output-producing Orka
submissions write observations in their single atomic store commit.

## Non-goals

- Starting or supervising agent processes.
- Docker, worktree, or network isolation policy.
- Scheduling and retry policy.
- Review comments, suggested edits, or deciding who may accept.
- Requiring Orka, Driva, or Nota for normal CLI/library use.
