# Linka2 merge-readiness tasks

This backlog covers what the `linka2` rebuild has to answer before it replaces
`linka` on `main`. `linka2` is a better library in most respects — one memoized
evaluation pass, a bad record as a state rather than a query failure, no
presentation inside the crate — and none of these tasks argue with that design.
They cover capabilities and guardrails that `main` has and `linka2` does not,
plus the practical work of landing it.

References are `branch:path:line`. Findings marked *(verified)* were reproduced
by running code against `linka2`, not read off the design.

Tasks are ordered by dependency: correctness of the derived model first, then
data compatibility, then the guardrails, then porting and coverage.

## 1. Put the publication guard back in the library

`main`'s `CandidateStore::publish` refuses to publish unless the candidate is
accepted, is still the node's current result, and reads as integration
`Accepted` (`main:linka/src/candidate/operations.rs:199`, via `require_current`
at `:221`). `linka2`'s `ops::publish` only fast-forwards
(`linka2:linka/src/ops/submit.rs:346`); the accepted check lives in the CLI
(`linka2:linka-cli/src/main.rs:474`).

*(verified)* Calling `ops::publish` on a candidate whose current review
concluded `rejected` returns `Ok(())` and moves `refs/heads/<target>` onto the
artifact. The node afterwards reads `integration: Rejected` while the work is
merged.

Orka consumes the library, not the CLI, so under `linka2` it can publish
rejected or undecided work with no error. This is the one finding here that
loses a safety property rather than a signal, and the crate split is what makes
it easy to get wrong: "no presentation decision inside the library" must not
become "no policy decision inside the library".

- [ ] Move the decision check into `ops::publish`: refuse anything whose derived
  decision is not `Accepted`, with the decision in the error message.
- [ ] Refuse a candidate that is no longer its node's current result, matching
  `main`'s `require_current`. Decide explicitly whether a superseded-but-accepted
  candidate is publishable; if it is, say so in `DESIGN.md` and drop this check.
- [ ] Take the `Graph` (or a decision argument) rather than re-deriving inside
  `publish`, so the caller pays for one pass and the check cannot disagree with
  what the caller displayed.
- [ ] Reduce the CLI handler to argument parsing and wording.

Acceptance criteria:

- No sequence of library calls publishes a candidate that is pending, rejected,
  or superseded.
- The CLI holds no publication policy the library does not enforce.

Tests:

- [ ] `publish` on a pending candidate fails and moves no ref.
- [ ] `publish` on a rejected candidate fails and moves no ref.
- [ ] `publish` on an accepted candidate is idempotent (already covered).
- [ ] `publish` on an accepted candidate whose node has since recorded a new
  result behaves as `DESIGN.md` says, whichever way that is decided.

## 2. Decide what happens to published output that is later modified

`main` checks drift for a published candidate against the target branch:
`vcs.drift(&output.id, Some(&target))` at `main:linka/src/ops/state.rs:170`. So
if the files a node produced are edited on the target branch afterwards, the
node goes stale with `OutputDrifted`.

`linka2` skips drift whenever a candidate exists —
`if let (Some(output), None) = (&result.output, candidate)` at
`linka2:linka/src/graph.rs:657` — on the grounds that the artifact commit is
immutable. The commit is, but the files it placed on the target branch are not.
The consequence is that every review-gated node is permanently `current` no
matter what happens to its output later, while direct results are still checked
against the working tree. For a graph whose purpose is "whether that evidence
still holds", the review-gated path is the one that matters most.

- [ ] Decide the intended semantics and write them into `DESIGN.md`'s staleness
  section, replacing "a result backed by a candidate never drifts".
- [ ] If drift should apply after publication, restore it for candidates whose
  derived integration is `Published`, comparing the artifact against the target
  branch rather than the working tree. Keep it out of the `pending`/`accepted`
  cases, where absence from the target is integration and not staleness.
- [ ] Keep it inside the memoized pass: the comparison is one `drift` question
  per (artifact, target) pair, so `MemoizingVcs` already collapses it.
- [ ] If drift should *not* apply, say why in `DESIGN.md` and note what does
  detect a reverted or overwritten merge instead.

Acceptance criteria:

- `DESIGN.md` answers, without inference, whether a merged output being changed
  on the target branch invalidates the node that produced it.
- Whatever the answer, direct results and candidate-backed results are treated
  consistently or the difference is justified in writing.

Tests:

- [ ] A published node whose output files are modified on the target branch
  reports the documented state (stale, or explicitly current).
- [ ] A pending or accepted candidate never reports `OutputDrifted`.

## 3. Stop a stale review from reverting a published decision

`Graph::decide` skips stale verifications (`linka2:linka/src/graph.rs:538`, via
`verification_is_stale` at `:575`), so a review that goes stale stops speaking
for its candidate and the decision falls back to `Pending`.

*(verified)* Complete a node, propose it, accept it, publish it — the node is
`Complete` with integration `Published`. Then edit the *review* node's own
description, which is enough to make its result stale. The source node becomes:

```
Known { outcome: Succeeded, currency: Current, integration: Pending,
        staleness: [], blockers: [] }   ->  workability AwaitingIntegration
```

The node's own result is still current and its artifact is still an ancestor of
the target branch, but it is no longer complete, and it is not redispatched
either — it sits in `awaiting-integration` until someone re-reviews it. A typo
fix on a review task unsettles finished, merged work.

The rule that a stale review does not speak for a candidate is sound for a
*pending* decision. It is the interaction with publication that is wrong, and it
contradicts the spirit of `linka2`'s own rule 1 — completion as a fact about a
node's own result — which `DESIGN.md` defends at length for the dependency case.

- [ ] Make ancestry authoritative once it holds: if the artifact is an ancestor
  of the target branch, integration is `Published` regardless of the current
  decision. Git is the record of what was published, exactly as the design
  argues for retry safety.
- [ ] Reconsider whether definition staleness on a review should silence its
  conclusion at all, as against staleness in what it *reviewed* (the source
  node's definition, result, or artifact). The latter means the review no longer
  covers this candidate; the former only means its own task text moved.
- [ ] Write the chosen rule into the integration table in `DESIGN.md`, which
  currently has no row for "accepted, then the review went stale".

Acceptance criteria:

- No edit to a review node can move a published node out of `Complete`.
- A pending candidate whose only review has gone stale still reads `Pending`.

Tests:

- [ ] Publish, then edit the review's description: the source node stays
  `Complete` with integration `Published`.
- [ ] Accept without publishing, then edit the review's description: the
  candidate reverts to `Pending` (the current behaviour, kept deliberately).
- [ ] Publish, then change the source node's definition: the node is stale and
  ready, by rule 1 and unchanged from `main`.

## 4. Give disagreeing reviews a resolution path

Two current reviews that decide one candidate differently make
`Graph::decide` return `Err`, which surfaces as an `Error` state on the
candidate's **source node**.

*(verified)* One `accepted` review and one `rejected` review of the same
candidate yield:

```
source state: Error { message: "candidate `…` has current verifications that disagree" }
source workability: Error
```

An `Error` node is never ready, never complete, and blocks its dependents. So
two reviewers doing their job and disagreeing — an ordinary outcome, not
corruption — wedges that branch of the graph, and no operation resolves it:
`linka2` has no `accept`/`reject`, so the only exits are resubmitting one
review's result or editing a review's description to make it stale. `main` made
this unambiguous by construction, because `accept <candidate> <verification>`
named the authorizing review; dissent was then reported by `check` as a problem
without breaking evaluation.

- [ ] Decide whether disagreement is corruption or a normal state needing a
  tie-break. If it is normal, add a rule — most recent conclusion wins, or
  rejection wins, or a designated deciding review — and document it.
- [ ] If it stays an error, at minimum keep it off the source node: report it on
  the candidate and via `check`, and leave the source node evaluable so the rest
  of the graph and its dependents are unaffected.
- [ ] Give the resolution an operation rather than requiring a hand-edit or a
  resubmitted review. Whatever it is, it must be one commit under the mutation
  lock like every other fact writer.
- [ ] Document the chosen rule beside the "two current verifications disagreeing
  is a corrupt graph" sentence in `DESIGN.md`, which currently states the
  condition without stating the remedy.

Acceptance criteria:

- Two honest, disagreeing reviews leave the graph queryable and offer a
  documented way forward.
- Whatever the rule, `check` reports the disagreement.

Tests:

- [ ] Accepted + rejected on one candidate produces the documented state.
- [ ] Dependents of the source node are not blocked by a review disagreement,
  unless that is the documented intent.
- [ ] The resolution operation produces the expected decision in one commit.

## 5. Version the store format and stop silently misreading old stores

Every schema constant is still `1` (`linka2:linka/src/model.rs:17` and the
consts above it), but the on-disk format changed incompatibly:

| record | `main` | `linka2` |
| --- | --- | --- |
| candidate | `candidates/<id>/` directory, has `state` | `candidates/<id>.toml`, no `state`, `deny_unknown_fields` |
| observed context | `observations/<blob-id>.toml`, append-only | `observed-context.toml`, replaced wholesale |
| attachment | hash-addressed directory | `attachments/<namespace>/<key>/` |

*(verified)* Pointing `linka2` at a `main`-format store:

- `check` reports `candidates/<id>: not a candidate record` — correct and useful;
- but the node's derived state reads `integration: NotRequired` and workability
  `Complete`. The accepted candidate is invisible, so review-gated work
  evaluates as directly-applied work that needs no integration;
- and the old `observations/` directory is ignored with **no problem reported at
  all**, so observed context pins silently vanish and the node reads `current`
  when its context has changed.

The root cause is that discovery problems stop at `check`:
`Store::load_candidates` and `list_nodes` return them
(`linka2:linka/src/store.rs:400`, `:430`), `Graph::load` keeps them
(`linka2:linka/src/graph.rs:56`, exposed at `:179`), and only `check` reads them
(`linka2:linka/src/check.rs:24`). Evaluation itself answers confidently from an
incomplete record set. This is the one place where "a bad record is a state, not
a query failure" is not carried through: an unreadable *node* becomes `Error`,
while an unreadable *candidate* becomes silence.

- [ ] Bump `CANDIDATE_SCHEMA`, `OBSERVATION_SCHEMA`, and `ATTACHMENT_SCHEMA`, and
  make the readers reject a record whose schema they do not know with a message
  naming the version, so an old store is diagnosed rather than half-read.
- [ ] Detect `main`-era layout explicitly: a `candidates/<id>/` directory, a
  `nodes/<id>/observations/` directory, and hash-named attachment directories
  are each a reported problem, not an unrecognised entry to skip.
- [ ] Carry discovery problems into evaluation. A node whose candidate set is
  damaged, or that has an unreadable observed-context or attachment record, must
  evaluate to `Error` rather than to a state derived from what happened to parse.
- [ ] Write a migration — a one-shot `linka migrate` or a documented conversion —
  or state in `DESIGN.md` that `linka2` stores are new stores and old ones are
  not readable. Either is defensible; silence is not.
- [ ] Decide whether `NodeMeta.extensions` (`main:linka/src/model.rs`, flattened
  namespaced application metadata) stays dropped. Nothing in the workspace uses
  it, so the only cost is that a `node.toml` carrying extra keys now fails
  `deny_unknown_fields` instead of round-tripping. If it stays dropped, the
  migration has to say so.

Acceptance criteria:

- No `main`-format store produces a confident wrong answer from any query.
- A store `linka2` cannot fully read is either migrated or refused, and in both
  cases says which format it found.
- `check` and node evaluation agree about which records are damaged.

Tests:

- [ ] A `main`-format candidate directory makes its source node `Error`, not
  `Complete`/`NotRequired`.
- [ ] A `main`-format `observations/` directory is reported as a problem.
- [ ] An unreadable candidate file makes its source node `Error` while every
  other node's state is unchanged.
- [ ] The migration, if written, converts a fixture store built by `main` and
  the result evaluates identically under both implementations.

## 6. Restore the small write-time guardrails

Two checks `main` enforced that `linka2` dropped along with `accept`/`reject`:

- **Rejection notes.** `main:linka/src/candidate/operations.rs:156` refuses a
  rejection with empty notes. `linka2`'s `verify --outcome rejected` accepts
  none, so a rejection can carry no reason at all — the one conclusion where the
  reason is the useful part.
- **Decision provenance.** `main`'s `CandidateState::{Accepted, Rejected}`
  recorded `decided_at_ms`, `author`, `notes`, and `verification`. Under
  derivation the first three come from the deciding review's own result, which
  is strictly better, but only if something exposes them together. `Candidate`
  currently reports its decision without saying who reached it or when.

- [ ] Require non-empty notes for `Conclusion::Rejected` in `submit`, so it holds
  for every front end and not just the CLI.
- [ ] Have the decision query return the deciding review's id, author, and
  timestamp alongside the conclusion, rather than the bare
  `CandidateDecision`. `Graph::decision` already visits exactly those records.
- [ ] Consider the same for `Abandoned`, which is also a claim about why nothing
  was decided.

Acceptance criteria:

- A rejection without a reason cannot be recorded through any interface.
- Given a decided candidate, one call answers "who decided this, when, and what
  did they say" without the caller re-walking the review nodes.

Tests:

- [ ] `submit` with `Rejected` and empty notes fails and writes nothing.
- [ ] A decided candidate reports its deciding review, author, and timestamp.

## 7. Report an acceptance that can no longer be published

`main` recorded `target_previous` on acceptance and read integration by
comparing it against the branch's current tip, so a target that had moved
without containing the artifact was an error naming both commits
(`main:linka/src/candidate.rs:91`). `linka2` records no such pin, and by design
a moved target is not an integrity problem.

*(verified)* With an accepted candidate whose target branch has advanced
independently, `linka2` reports integration `Accepted` and workability
`AwaitingIntegration`; `publish` then fails cleanly with
`… cannot fast-forward `main`: other is not contained in commit-1`.

The design's argument for this is good — publication should not make a node
permanently unreadable, and a moved branch genuinely is not corruption. The gap
is only visibility: nothing reports the stuck acceptance until someone attempts
publication, and `awaiting-integration` work is not redispatched, so it can sit
indefinitely with no signal.

- [ ] Add the question to `check --artifacts`: for each accepted, unpublished
  candidate, whether a fast-forward is still possible, reported as a problem
  when it is not.
- [ ] Consider surfacing it in the state model — an `IntegrationStatus` that
  distinguishes "accepted, fast-forward still possible" from "accepted, target
  diverged" — or explicitly decide that `check` is the right and only place.
- [ ] Make the CLI's candidate and blocked listings show it either way, since
  this is the state a human most needs to see.

Acceptance criteria:

- An acceptance that can no longer be published is discoverable without
  attempting publication.
- Reading state still never fails because a branch moved.

Tests:

- [ ] An accepted candidate whose target diverged is reported by `check`.
- [ ] Reading the source node's state succeeds and stays `AwaitingIntegration`.

## 8. Port the consumers

`linka2` changes only `linka/` and the new `linka-cli/`. `orka` and `linka-tui`
still compile against the old API: `cargo check -p linka -p linka-cli` passes,
`cargo check -p orka -p linka-tui` fails with roughly twenty unresolved imports
and missing functions. The workspace does not build.

The mapping for the surface the two crates import today — every row below is
something the compiler currently rejects, not the full list of API changes:

| `main` | `linka2` |
| --- | --- |
| `CandidateRecord`, `CandidateState`, `CandidateStore` | `Candidate` + `Graph::decision` |
| `ResultOutcome`, `VerificationOutcome` | one flat `Outcome` with `family()` |
| `ResultSubmission`, `VerificationSubmission` | `Submission` + `Conclusion` |
| `ProducerEvidence` | `Namespaced` (`toml::Value`, not `serde_json::Value`) |
| `NewNodeAttachment`, `NodeAttachment` | `NewAttachment`, `Attachment` |
| `ArtifactStore`, `BranchStore` | one `Vcs` trait |
| `ops::snapshot_work` | `ops::snapshot` |
| `ops::submit_result_with_attachments`, `submit_verification`, `capture_submission`, `submit_captured_execution_with_attachments` | `ops::submit` |
| `ops::add_verification` | `ops::add(.., verifies)` |
| `ops::record_context_observation` | `ops::record_observed_context` (one file, current result only) |
| `ops::node_state`, `ops::ready_nodes` | `Graph::state`, `Graph::ready` |
| `ops::short` | `linka-cli::render::short` (now `pub(crate)` in the library) |

- [ ] Port `orka` to `Graph` and `ops::submit`. Load one `Graph` per orchestration
  pass rather than calling a per-node query in a loop — this is where the
  memoized pass pays off, and a mechanical translation would keep the old
  quadratic shape.
- [ ] Give `orka` its own publication policy check if task 1 lands the guard in
  the library, or delete its copy if the library now refuses.
- [ ] Port `linka-tui`, which reads `CandidateState` for display: the decision is
  now derived, and the display wording is the front end's own (the library
  deliberately no longer provides it).
- [ ] Decide whether `orka` and `linka-tui` need the shortening helpers, and if
  so where they live — a third copy in each front end, or a shared render crate.
- [ ] `cargo check --workspace` and `cargo test --workspace` clean.

Acceptance criteria:

- The whole workspace builds and its tests pass.
- No consumer reimplements a rule the library now owns, and no consumer relies
  on a rule the library dropped.

## 9. Cover the behaviour these tasks changed

`main` has 85 library unit tests; `linka2` has 53 plus a 4-test real-git
workbench suite. That is not a straight downgrade — `linka2`'s fake models
commit parentage where `main`'s `is_ancestor` is `ancestor == descendant`
(`main:linka/src/vcs.rs:249`), which makes the normal post-merge case genuinely
testable for the first time, and the workbench suite covers the git seam the
fake abstracts. But `main`'s 532-line `candidate/tests.rs` has no direct
replacement, and every finding above was reachable because nothing asserted the
property.

- [ ] Land the per-task tests listed above, each as an assertion about state
  rather than about a return value.
- [ ] Add the properties `DESIGN.md` already claims are worth asserting directly
  and that no test currently covers: publication is idempotent under a moved
  target; an accepted-then-superseded candidate behaves as documented; a review
  disagreement leaves other nodes' states untouched.
- [ ] Read `main`'s `candidate/tests.rs` for cases the rebuild has no equivalent
  of, and port the ones that still describe intended behaviour.
- [ ] Extend the workbench suite with a publication case against real git,
  including a target branch that moved.

Acceptance criteria:

- Each finding in tasks 1–7 has a test that fails against the current `linka2`
  and passes after the fix.
- No behaviour documented in `DESIGN.md`'s truth or integration tables is
  unasserted.
