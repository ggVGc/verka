# Human review gating and named candidate branches

Status: historical workflow being adapted to Nota. Nota owns comments,
suggested edits, decisions, and follow-up requests behind its `ReviewStore`
trait. Linka-specific nodes and branches are adapter behavior; Orka does not
automatically create or interpret reviews.

## 1. Purpose

Every machine-produced project change requires explicit human approval before
it reaches `main`. Automated tests are part of responsible human review, but
linka does not yet model automated verification as a separate graph concept.

Git remains the source of truth for project content and history. Linka uses
ordinary named branches and worktrees for candidates and review suggestions,
while graph nodes record why those branches exist and how their contents were
reviewed.

## 2. Core rules

1. Every implementation attempt runs in a worktree on a unique named branch.
2. Candidate branches are permanent historical records. Linka never reuses,
   resets, or automatically deletes them.
3. Completing machine work does not integrate it into `main`.
4. A completion that produces a project commit automatically creates a review
   node assigned to a human.
5. A review is tied to one exact candidate branch, commit, and result version.
6. Human acceptance authorizes integration of exactly the reviewed content.
7. Rejected work remains inspectable and makes the implementation eligible for
   another attempt on a new candidate branch.
8. Reviewer edits are suggestions. They are made on a separate review branch
   and are inputs to later implementation work, never integrated directly.
9. Any change needed after approval, including a non-trivial rebase or conflict
   resolution, produces a new candidate and requires another review.

## 3. Branch and worktree model

Implementation attempts use branches in this namespace:

```text
linka/candidates/<attempt-id>
```

`attempt-id` is an opaque ULID generated before the branch or worktree is
created. The attempt log and result metadata record both the ID and branch.
The worktree is created with the branch checked out rather than at detached
HEAD. Removing the worktree does not remove the branch.

Review suggestion work uses:

```text
linka/reviews/<review-node-id>
```

The review branch starts at the reviewed candidate commit and is created only
when the reviewer elects to edit files. The original candidate branch remains
unchanged.

Branches are project history, not linka-store content. Preserving or sharing
old candidates therefore means preserving or publishing the corresponding
project branches. Linka should eventually provide explicit synchronization
help, but must not push unfinished work without user authorization.

## 4. Graph lifecycle

An implementation node is the logical unit of work. It may have several
attempts over time, each represented by durable attempt metadata and a distinct
candidate branch. Each project-producing attempt gets its own immutable review:

```text
implementation
  +-- attempt 1 / candidate 1
  |     +-- review 1: rejected
  +-- attempt 2 / candidate 2
        +-- review 2: accepted
              +-- integrated into main
```

The first implementation may continue to store the latest result on the
implementation node, provided every review pins the exact older result version,
candidate commit, and candidate branch. Replacing the latest result must never
make an earlier reviewed candidate undiscoverable.

## 5. Review nodes

A review node is created automatically after a machine completion with an
output commit. It is assigned to a human and records a first-class `review_of`
relationship to the implementation node. Its definition pins:

```toml
review_of = "implementation-node-id"
attempt_id = "01..."
candidate_branch = "linka/candidates/01..."
candidate_commit = "abc123..."
reviewed_result = { metadata = "...", notes = "..." }
```

The exact schema may differ, but these facts must be structured rather than
encoded only in prose.

Completing a review records `review_decision = "accepted"` or `"rejected"`.
Both are successfully completed reviews; rejection is not represented by the
generic failed-work outcome. Rejection notes live in `result.md`. When proposed
edits exist, the result also records their branch and commit.

## 6. Acceptance and integration

Human acceptance is authoritative. The acceptance operation verifies that the
recorded candidate branch still resolves to the exact reviewed commit, then
publishes that content to the configured target branch.

The initial implementation only accepts a candidate when the target can be
fast-forwarded to the reviewed commit. If the target has moved, acceptance
does not silently rebase, merge, invoke an agent, or otherwise change approved
content. Linka instead creates or requests follow-up implementation or
reconciliation work. Its result is a new candidate with a new review.

After successful publication, the review and implementation result record the
integrated commit, target ref, and prior target commit. Candidate and review
branches remain as history.

Because the project and linka stores are separate repositories, acceptance
is a recoverable transaction rather than an atomic write. Linka commits a
publication intent before moving the target. It then closes the review, updates
the implementation view, and marks the intent complete in one store commit.
`recover-publication` safely resumes either before or after the target move.

## 7. Rejection and rework

Rejecting a review records comments and optional suggestion-branch metadata.
It makes the logical implementation eligible for another machine attempt even
though its latest implementation result remains historically valid as the
thing that was reviewed.

The next worker receives the rejection notes and any suggestion commit as
context. It uses a fresh candidate branch. A new completion creates a new
review node; an earlier review is never reopened or overwritten.

## 8. Implementation sequence

1. Use named candidate branches for all execution worktrees and record their
   attempt metadata.
2. Add structured review relationships, candidate pins, and decisions.
3. Automatically create a human review node after project-producing completion.
4. Remove automatic publication from machine completion.
5. Add accept and reject operations and derive implementation readiness from
   the latest review.
6. Add optional review branches/worktrees for proposed edits.
7. Feed rejected-review material into subsequent attempts.
8. Add consistency checks and end-to-end lifecycle tests.

The initial concurrency policy permits only one unfinished attempt for a
logical implementation node. Parallel alternatives use distinct child nodes.
Execution start is serialized around authorization and durable attempt
creation; `--force` does not bypass this identity constraint.
