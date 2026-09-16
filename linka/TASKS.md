# Linka correctness and scope tasks

This backlog brings Linka's implementation into line with `DESIGN.md`: Linka
owns versioned graph facts and their derived state, while execution, worktree,
retry, review, and publication policy belong to applications such as Orka.

Tasks are ordered by dependency. Semantic correctness comes before API cleanup.

## 1. Specify the graph-state contract

- [x] Replace the ambiguous single `status` concept in `DESIGN.md` with three
  independent dimensions:
  - recorded outcome: open, succeeded, or failed;
  - currency: current or stale;
  - workability: complete, ready, or blocked.
- [x] Document the authoritative rules:
  - a node is complete only when it has a successful result covering its
    current definition, consumed inputs, context, and output;
  - a node is ready when it is not complete and every `depends_on` node is
    complete;
  - otherwise it is blocked;
  - `derived_from` affects provenance and staleness but not blocking.
- [x] Specify how failure evidence behaves after definition or input changes.
- [x] Specify that successful results may only be accepted for ready nodes,
  while failed evidence may be recorded independently.
- [x] Specify that read, parse, and artifact-backend failures are errors rather
  than graph states.
- [x] Add a truth table covering open, failed, successful/current,
  successful/stale, ready, blocked, and corrupt nodes.

Acceptance criteria:

- `DESIGN.md` unambiguously answers whether any combination of result,
  staleness, and dependency state is complete, ready, or blocked.
- The documented rules do not depend on Orka attempts or scheduling policy.

## 2. Introduce one authoritative derived-state API

- [x] Add structured types for recorded outcome, currency, blockers, and
  staleness reasons.
- [x] Add a `NodeState` type with `is_complete`, `is_ready`, and `is_blocked`
  helpers.
- [x] Implement a single fallible `node_state` derivation that reads the
  definition and result, checks all pins and output validity, evaluates
  blockers, and returns structured state.
- [x] Reimplement `current_status`, `staleness`, `blockers`, `is_ready`,
  `ready_nodes`, and `unsettled` as projections over that derivation.
- [x] Move human-readable reason formatting into the CLI.
- [x] Deprecate APIs whose return types cannot represent stale or corrupt state.

Acceptance criteria:

- A successful result with any changed consumed input is not complete.
- All public state queries agree because they use the same derivation.
- Corrupt or unreadable facts produce errors, not `open`, `ready`, or empty
  blocker/staleness lists.

## 3. Make stale work selectable for rework

- [x] Change readiness so a successful-but-stale node is ready when its current
  `depends_on` dependencies are complete.
- [x] Keep a stale node blocked when any required dependency is incomplete.
- [x] Make dependents of stale nodes remain blocked.
- [x] Present the previous stale result as evidence without treating it as a
  current completion.

Tests:

- [x] Changed definition makes a previously successful node stale and ready.
- [x] Changed dependency result/output makes its consumer stale and ready once
  dependencies are complete.
- [x] Changed context and drifted output make a node stale and ready.
- [x] A stale node with an incomplete dependency is blocked.
- [x] A dependent of a stale node is blocked.
- [x] A failed node is ready only when its dependencies are complete.

## 4. Stop converting failures into graph facts

- [x] Change derived queries to return `Result`.
- [x] Distinguish legitimate absence, staleness, corruption, and backend
  failure.
- [x] Treat a missing context file as staleness, but a permission or I/O error
  as an error.
- [x] Treat a proven-missing artifact as staleness, but an artifact-backend
  failure as an error.
- [x] Update list-oriented CLI commands to report per-node errors and exit
  nonzero if any node could not be evaluated.
- [x] Ensure `stale`, `ready`, and `blocked` never print a clean result after
  silently skipping errors.

Tests:

- [x] Malformed definitions and results are not reported as open or ready.
- [x] Context read failures are not reported as missing files.
- [x] Artifact lookup failures are not reported as drift or absence.
- [x] A missing target node is distinguished from an unreadable target node.

## 5. Add validated graph identifiers and project paths

- [x] Introduce `NodeId` and `ProjectPath` newtypes.
- [x] Reject empty, absolute, traversal, control-character, and platform-prefix
  forms.
- [x] Define whether `.git` paths are always forbidden; default to forbidding
  them.
- [x] Normalize portable path separators before persistence.
- [x] Validate values at CLI parsing, public library entry points,
  deserialization, and store directory discovery.
- [x] Prevent symlink-based escape when reading working-tree files, or avoid it
  by resolving context through repository objects.

Tests:

- [x] Reject `..`, `../secret`, absolute paths, backslash traversal, `.git`
  internals, and symlinks escaping the project root.
- [x] Accept valid node IDs and nested project-relative paths.

## 6. Expand `linka check` into semantic fsck

- [x] Validate supported definition and result schema versions.
- [x] Validate node IDs, required files, paired result files, and normalized
  paths.
- [x] Retain edge checks for missing targets, duplicates, self-links, and
  `depends_on` cycles.
- [x] Validate unique consumed-node and context pins.
- [x] Validate that result pins correspond to declared relationship edges.
- [x] Require successful `depends_on` pins to contain successful result
  evidence.
- [x] Define and validate the weaker `derived_from` pin invariant.
- [x] Validate supported artifact schemes and repository identities.
- [x] Keep historical pin mismatches out of fsck: those are staleness, not
  corruption.
- [x] Add an artifact-aware check mode that verifies referenced commits and
  retained output refs.
- [x] Keep `check` read-only; make any future repair operations explicit.

Acceptance criteria:

- Hand edits and merges cannot create parseable but semantically impossible
  results without `linka check` reporting them.
- Structural checking works without a project checkout; artifact checking is
  explicitly opt-in.

## 7. Add frozen work snapshots

- [x] Add a graph-owned `WorkSnapshot` containing the node ID, definition
  version, dependency/lineage pins, and explicit context pins.
- [x] Represent the project input revision as a generic artifact/project
  snapshot, separate from graph identity.
- [x] Add a `snapshot_work` operation that rejects unknown, blocked, corrupt,
  or unreadable nodes.
- [x] Permit snapshotting a stale node when its current dependencies are
  complete.
- [x] Keep attempt IDs, sessions, branches, worktree paths, and backend details
  out of snapshot types.

Tests:

- [x] Snapshots contain exact definition, dependency result/output, lineage,
  context, and project revision identities.
- [x] Blocked and corrupt nodes cannot be snapshotted.

## 8. Add compare-and-record result submission

- [x] Add `ResultSubmission`, carrying a frozen snapshot, outcome, optional
  output artifact, notes, author, and optional opaque producer evidence.
- [x] Add `submit_result`, which rechecks every frozen graph and context version
  immediately before writing.
- [x] Return structured conflicts for definition, dependency, context, and
  readiness changes.
- [x] Include the expected previous result version to prevent concurrent
  overwrites.
- [x] Perform snapshot revalidation and result replacement under a store
  mutation lock.
- [x] Reject a conflicting submission without overwriting the prior result.
- [x] Keep `complete` temporarily as a short-lived convenience wrapper that
  snapshots, captures, and submits within one call.
- [x] Document that long-running workers must use the explicit snapshot and
  submission operations.

Tests:

- [x] Reject submission after changes to definition metadata or description.
- [x] Reject submission after dependency definition, result, or output changes.
- [x] Reject submission after context changes or a new required dependency is
  added.
- [x] Reject concurrent submission when the expected previous result changed.
- [x] Preserve the previous result on every rejection.

## 9. Enforce valid successful results

- [x] Require a successful submission to match its snapshot and have all
  current `depends_on` nodes complete.
- [x] Forbid a successful `depends_on` pin with no result or with failed/stale
  evidence.
- [x] Separate required dependency pins from weaker lineage pins if that makes
  their invariants explicit.
- [x] Define `derived_from` behavior: it does not block readiness, but observed
  lineage versions participate in provenance and later staleness.
- [x] Make `respond` an output-free convenience over ordinary checked result
  submission rather than a special graph concept.
- [x] Continue allowing failed evidence to be recorded without falsely marking
  the node complete.

Tests:

- [x] A blocked node cannot receive a successful result.
- [x] Failed, stale, and result-less required dependencies prevent success.
- [x] Output-free success follows the same validation as output-producing work.
- [x] Lineage does not accidentally become a scheduling dependency.

## 10. Make context and artifact identity revision-based

- [x] Record the source project revision/tree on each context snapshot or on
  the containing work snapshot.
- [x] Check context through an explicit repository revision rather than the
  process's currently checked-out worktree.
- [x] Define currency relative to an explicit current project revision supplied
  by the caller.
- [x] Ensure dirty working-tree state does not silently redefine graph
  currency.
- [x] Populate `ArtifactRef.repository` from the verified project pairing rather
  than leaving it empty.
- [x] Reject artifacts from a different paired repository.

Tests:

- [x] Context captured in a linked worktree is checked correctly when the main
  checkout differs.
- [x] Identical blobs remain current across revisions.
- [x] Missing paths at the comparison revision become stale.
- [x] Artifacts from another repository are rejected.

## 11. Narrow Linka's VCS and storage interfaces

- [x] Split the broad `Vcs` trait into narrow graph-facing capabilities for
  store history, artifact inspection/retention, and context identity.
- [x] Remove branch, revision-resolution, worktree, and publication methods
  from Linka's public traits.
- [x] Move `Worktree`, candidate-branch helpers, worktree cleanup, ref
  publication, and their tests into Orka or a project adapter owned by Orka.
- [x] Keep only generic artifact facts and inspection in Linka.
- [x] Add dependency/architecture checks ensuring Linka does not import Orka,
  review, worktree, session, or publication concepts.

Methods to move out of Linka include:

- `current_branch`
- `resolve_revision`
- `tree_id` where only orchestration uses it
- `ref_commit`
- `publish_fast_forward`
- `create_worktree`
- `worktree_clean`
- `remove_worktree`

Acceptance criteria:

- Linka builds and tests without worktree or publication behavior.
- Orka owns the project lifecycle operations it consumes.

## 12. Move execution logs and transcript interpretation to Orka

- [x] Move `work.jsonl`, `read_work_log`, `open_work_log`, and
  `commit_work_log` to Orka's attempt/session storage.
- [x] Replace transcript-specific `amend_context` with a neutral context
  observation input, if Linka needs such an operation at all.
- [x] Prefer immutable observation records keyed by result version over
  rewriting an existing result after completion.
- [x] Make Orka responsible for mining transcripts and submitting observations.
- [x] Confirm observations cannot change a definition or silently replace the
  result they refer to.

Acceptance criteria:

- Linka neither knows nor cares that context observations came from a session
  transcript.
- Interaction logs do not participate in Linka graph versions or derived state.

## 13. Detect incomplete cross-repository completion

- [x] Keep submission journals and procedural recovery state out of Linka.
- [x] Mark output commits with their `Linka-Node` trailer.
- [x] Report the output commit explicitly if project capture succeeds but
  recording the Linka result fails.
- [x] Refuse CLI operations when project `HEAD` is a Linka output that has
  never been recorded in store history.
- [x] Accept historical outputs whose current result has since been replaced.
- [x] Do not silently discard an output artifact when graph submission fails.

Tests:

- [x] Detect an unrecorded Linka output at project `HEAD`.
- [x] Accept an output found in committed store history.
- [x] Include the dangling output commit in an immediate completion error.
- [x] Handle two submissions from the same snapshot without overwriting the
  winner.

## 14. Update CLI state presentation

- [x] Show complete, ready, blocked, stale prior result, failed prior attempt,
  and corruption as distinct conditions.
- [x] Include concise structured reasons for readiness, blocking, and
  staleness.
- [x] Keep `stale` as a historical-result query even though stale nodes also
  appear in `ready` or `blocked`.
- [x] Ensure all commands use the authoritative `NodeState` derivation.
- [x] Define stable nonzero exit behavior for corruption, backend failure,
  conflicts, failed checks, and unsettled nodes.

Example output shape:

```text
node-...  complete
node-...  ready (previous result stale: dependency A changed)
node-...  ready (previous attempt failed)
node-...  blocked by A: stale
node-...  error: malformed result.toml
```

## 15. Version the stored schema

- [x] Introduce explicit supported schema versions for definitions, results,
  snapshots, and any observation records.
- [x] Require readers and writers to use the current schema exactly.
- [x] Keep work completion and verification conclusions distinct in the stored
  result model.
- [x] Require candidate decisions to identify their authorizing verification.

Acceptance criteria:

- Unsupported schemas fail explicitly instead of being interpreted.
- Every newly initialized project uses the sole current format.

## Final verification

- [x] Add an end-to-end test covering: create graph, snapshot ready work,
  change an input, reject stale submission, resnapshot, submit successfully,
  make a dependency change, and select the consumer for rework.
- [x] Add an end-to-end crash-recovery test spanning artifact capture and graph
  submission.
- [x] Run `cargo test` and clippy for Linka and every in-repository consumer.
- [x] Verify Linka's public API contains no attempt, session, worktree, review,
  retry, or publication policy.
- [x] Verify every statement in `linka/DESIGN.md` has a corresponding test or
  documented external responsibility.

# Simplification backlog

The following tasks are pending work from the simplification review. The
completed checklist above is historical: where it conflicts with the current
candidate/review/publication model, use `DESIGN.md` and the current callers as
the baseline. In particular, task S7 deliberately revisits the capability-trait
split from task 11; it does not move publication out of Linka.

Preserve versioned definitions and results, dependency and lineage semantics,
staleness, checked submissions, candidate verification and publication, and
durable producer evidence. Preserve mutation locking, clean-store checks,
exact input pins, artifact retention, and compare-and-swap publication.

Suggested sequence: S1 and S3 first; S5 before S2; S3 before S4. S6 and S7
can be implemented independently. Keep each task reviewable and distinguish
actual code deletion from code moved between crates. Do not remove tests just
to reduce line counts. Run formatting checks and the relevant crate tests and
clippy checks; expand validation to consumers when changing their APIs.

## S1. Make verification submission the sole candidate-decision writer

Problem:

`submit_verification` already writes the review result and matching candidate
decision in one mutation. `CandidateStore::accept` and `reject` provide another
write path with their own validation and retry rules. CLI/TUI and Orka callers
can therefore repeat a decision that submission has already recorded.

Primary code:

- `src/ops/submit.rs`: verification submission and prepared candidate decisions.
- `src/candidate/operations.rs`: `accept`, `reject`, `require_verification`.
- `src/main.rs`, `../linka-tui/src/app.rs`: candidate decision commands/actions.
- `../orka/src/review.rs`, `../orka/src/candidate.rs`, and Orka frontends.

Implementation:

- [x] Inventory callers and distinguish initial review submission, retry after
  successful submission, and attempts to decide manually edited/legacy records.
- [x] Make checked verification submission the only normal operation that
  changes a candidate from pending to accepted or rejected.
- [x] Remove redundant follow-up mutations from Orka's review flow. Preserve
  recovery when the store commit succeeded before Orka recorded completion.
- [x] Remove standalone decision APIs and UI actions where callers can migrate
  together. If compatibility commands remain, make them read-only checks of
  the recorded decision; document that they cannot change author or notes.
- [x] Define matching retry identity explicitly: candidate, verification, and
  conclusion must agree. Preserve rejection-note requirements and refuse a
  conflicting decision without writing anything.
- [x] Delete validation and decision-building code made unreachable by the
  single writer. Retain integrity checking for hand edits and old records.
- [x] Keep publication separate and retain the accepted target's previous
  commit. Update API docs, command help, and `DESIGN.md` where necessary.

Acceptance criteria:

- One successful accepted/rejected verification submission creates one store
  commit containing both facts. Abandonment creates no candidate decision.
- Retrying after that commit creates no additional decision commit and cannot
  change the original deciding verification, author, notes, or target pin.
- A stale review, wrong candidate pin, or conflicting decision is rejected.
- All in-repository callers compile and the ordinary review-to-publication
  workflow remains available without a second decision command.

Validation:

- [x] Cover accepted, rejected, abandoned, stale, and wrong-artifact reviews.
- [x] Exercise recovery immediately after the verification store commit.
- [x] Verify repeated decisions and conflicting retries preserve stored bytes
  and commit count. Run Linka, Linka TUI, Orka, and affected Orka frontend tests.

Dependencies: none; keep the public submission changes compatible with S5.

## S2. Submit result, candidate, attachments, and observed context together

Problem:

Orka currently submits a result and attachments, registers a candidate, and
records context observations in separate Linka mutations. Recovery must finish
these steps individually, and intermediate graph state lacks some facts.

Primary code:

- `src/ops/submit.rs`, `src/ops/mutate.rs`, `src/candidate/operations.rs`.
- `src/model.rs`, `src/store.rs`, `src/ops/state.rs`, `src/ops/check.rs`.
- `../orka/src/linka_work.rs`, `../orka/src/engine.rs`, `../orka/src/input.rs`.

Implementation:

- [x] Extend the submission envelope from S5 with optional candidate details
  and producer-neutral observed project paths. Keep Orka attempt interpretation
  and access-journal parsing in Orka.
- [x] Resolve observed identities from the frozen project input revision,
  validate normalized paths, and deduplicate them with declared context. Retain
  the existing handling of node-output paths and absent observed paths unless
  a separately documented correctness change is needed.
- [x] Keep discovered context distinct from the original frozen snapshot:
  do not pretend these paths were declared before execution. Validate their
  currency under the submission lock using an explicit comparison policy;
  handle paths also modified by the submitted output deliberately.
- [x] Prepare and validate result, attachments, and candidate before the first
  store write. Compute the candidate's exact result version from the same
  serialized bytes that will be persisted, including optional notes.
- [x] Write all store facts under one mutation lock and commit once. Return
  enough result/candidate identity for Orka to seal the attempt directly.
- [x] Simplify Orka recovery to look up and validate the committed submission
  rather than separately registering candidates and adding observations.
  Detect conflicting external identities rather than silently reusing them.
- [x] Audit whether any caller requires genuinely late context observations
  or standalone registration. Remove those write paths only if all supported
  use cases are covered; otherwise keep a narrowly scoped extension API.
- [x] Specify compatibility before changing storage. Folding historical
  observation files into results changes result hashes and downstream pins:
  do not rewrite them silently. Prefer a documented legacy-reader transition
  or an explicit migration that accounts for every affected reference.
- [x] Update `DESIGN.md` and remove obsolete recovery branches, observation
  storage code, and schemas only when compatibility permits.

Acceptance criteria:

- A project-producing successful submission exposes its result, attachments,
  candidate, and observed-context facts in one committed store state.
- Validation conflicts leave all store facts untouched. A write/commit failure
  may leave a dirty store, which continues to block later mutations; do not
  promise filesystem rollback that the current transaction model lacks.
- Graph-only success, failure evidence, and verification still work without
  candidate registration. Candidate registration requires a successful work
  result with an exact output artifact.
- Retrying after a successful store commit does not duplicate records or
  re-run work. Existing durable evidence and historical pins remain readable
  according to the documented compatibility policy.
- Artifact capture/import and branch publication remain separate repository
  operations. Their interruption and retention guarantees remain explicit.

Validation:

- [x] Test graph-only, output-producing, failed, and verification submissions.
- [x] Test duplicate/invalid context, attachment conflicts, external-identity
  conflicts, wrong repository artifacts, and stale snapshots before any writes.
- [x] Test interruption after artifact capture, during store writes, and after
  the store commit but before Orka seals the attempt.
- [x] Test observed-input changes, output-overlapping reads, and legacy
  observation records; verify no silent changes to historical result versions.
- [x] Run Linka and Orka integration/recovery tests and affected frontend tests.

Dependencies: S5; S3 is recommended for exact record-version handling.

## S3. Read parsed records and versions through one authoritative loader

Problem:

Pinning and evaluation combine `read_node`/`node_version` and
`read_result`/`result_version`, rereading the same files. Optional result readers
also disagree: `read_result` checks for orphaned notes, while
`current_result_version` checks only metadata existence.

Primary code: `src/store.rs`, `src/ops/mod.rs`, `src/ops/state.rs`,
`src/ops/submit.rs`, and their library/frontend callers.

Implementation:

- [x] Introduce small loaded-definition and loaded-result records containing
  parsed metadata, prose, and the version hashes of the exact bytes read.
- [x] Hash original bytes rather than reserialized TOML. Preserve the version
  distinction between missing notes and an existing empty notes file.
- [x] Centralize optional-result handling: absent metadata and absent notes
  means no result; orphan notes, unsupported schema, parse failures, and I/O
  failures produce contextual errors.
- [x] Replace separate reads and hashes in pinning, submission, evaluation,
  and presentation. Retain thin compatibility methods only where useful.
- [x] Remove duplicate readers once consumers migrate. Keep paths and the
  current on-disk schema unchanged.
- [x] Document that matching parsed content and hashes does not itself make
  a multi-file read atomic against concurrent writers; preserve existing
  mutation-lock boundaries and avoid claiming a stronger snapshot guarantee.

Acceptance criteria:

- Each loaded file is read once for both parsing and hashing.
- Valid existing stores produce identical definition and result versions.
- Every optional-result API agrees on absence and corruption.
- Callers no longer need to coordinate separate metadata/prose/version reads.

Validation:

- [x] Compare hashes for existing fixtures, including TOML comments/formatting,
  empty notes, absent notes, and non-ASCII prose.
- [x] Cover orphan notes, malformed metadata, unsupported schemas, and read
  errors. Verify these cannot become open/ready state.
- [x] Run Linka tests and compile/test consumers touched by return-type changes.

Dependencies: none; provides the loading foundation for S4.

## S4. Reuse an operation-scoped graph view and state evaluation

Problem:

Whole-graph queries evaluate nodes separately, recursively revisiting shared
dependencies. Candidate lookup, reverse edges, and verification associations
are repeatedly rebuilt, especially during Linka TUI refresh.

Primary code: `src/ops/state.rs`, `src/ops/query.rs`,
`src/candidate/storage.rs`, `src/main.rs`, `../linka-tui/src/app.rs`.

Implementation:

- [x] Build an operation-scoped graph view from S3 loaded records, with maps
  for nodes, candidates by source/result, reverse edges, and verifications.
- [x] Memoize derived node states within the view. Keep an active traversal
  set separate from completed evaluations so cycles remain detectable.
- [x] Reuse candidate integration results and repeated backend lookups where
  their comparison inputs are identical. Define whether relevant Git refs are
  resolved once per view; do not claim filesystem/Git-wide atomic snapshots.
- [x] Route ready listings, settlement traversal, and frontend refresh through
  one view. Preserve simple one-node query entry points as adapters.
- [x] Preserve current error scope deliberately: list commands must report
  per-node failures, and an unrelated malformed record must not silently alter
  the behavior of a query that previously did not read it.
- [x] Discard the view after the operation. Create a fresh view for mutation
  revalidation under the lock; never reuse a frontend refresh as write authority.
- [x] Remove redundant scanning helpers after migration. Do not add a stored
  index, daemon, or persistent cache invalidation mechanism.

Acceptance criteria:

- Each reachable node is evaluated at most once per view; shared dependencies
  do not cause repeated recursive evaluations.
- Queries preserve ordering, assignment filters, dependency/lineage semantics,
  missing-node handling, and structured errors.
- A subsequent operation sees edits, new results, and moved target refs.
- Report both code-size changes and observed I/O/backend-call reductions;
  this task may improve complexity without reducing total lines immediately.

Validation:

- [x] Use diamond and layered shared-dependency graphs to verify evaluation
  reuse; include cycles, missing nodes, malformed records, and candidate errors.
- [x] Verify ready/blocked/stale/settled results match existing fixtures.
- [x] Change a result and a target ref between views and verify fresh results.
- [x] Measure deterministic read/backend-call counts on a representative graph;
  avoid flaky wall-clock performance assertions.

Dependencies: S3. Coordinate candidate loading with S2 if both are in progress.

## S5. Consolidate submission data and plumbing while preserving policies

Problem:

Public work and verification submissions and the internal recorded submission
repeat the same fields. Capture, conversion, validation, and retention behavior
are distributed across several entry points despite an existing shared writer.

Primary code: `src/model.rs`, `src/ops/submit.rs`, `src/ops/mutate.rs`,
`src/lib.rs`, and submission callers in Orka and the frontends.

Implementation:

- [x] Document a behavior matrix for `complete`, `respond`, `fail`, checked
  work submission, verification submission, and captured execution submission:
  readiness, dirty-tree policy, snapshot origin, lock lifetime, artifact capture,
  retention timing, attachments, and conflict/error reporting.
- [x] Introduce a common envelope for snapshot, notes, author, producer, and
  attachments with a typed payload that distinguishes work from verification.
  Verification payloads must not expose project-output fields.
- [x] Reuse the current checked writer instead of adding another submission
  engine. Consolidate field conversion and common result preparation.
- [x] Extract shared capture/message/artifact preparation only where policies
  agree. Keep explicit orchestration at entry points where lock or retention
  timing differs; avoid a collection of loosely related boolean flags.
- [x] Keep short-lived completion locked from its clean-store precondition
  through capture and result commit, including interrupted-completion checks.
- [x] Preserve `respond` on dirty projects and direct `fail` on non-ready work.
  Frozen long-running submissions retain their existing conflict checks; do
  not force direct failure recording through a ready-only snapshot operation.
- [x] Preserve artifact-retention behavior on acceptance, conflict, and capture
  failure, and include created output IDs in existing error paths.
- [x] Migrate callers and remove redundant payloads/constructors. If serialized
  public submissions change, provide an explicit compatibility policy without
  accidentally changing stored result or snapshot formats.

Acceptance criteria:

- Shared fields and checked result-writing logic have one authoritative form.
- Invalid work/review payload combinations remain unrepresentable or explicitly
  rejected before writes. Ordinary nodes cannot receive review conclusions.
- The entry-point behavior matrix is preserved, including deliberate differences
  between short-lived failure recording and frozen worker submissions.
- S2 can extend the common envelope without creating another submission family.

Validation:

- [x] Exercise the behavior matrix, especially dirty-tree responses, blocked
  direct failures, stale worker snapshots, graph-only success, and review output
  rejection.
- [x] Preserve race/conflict, orphan-output, retention, and attachment-atomicity
  regressions. Run Linka and Orka tests plus affected frontend checks.

Dependencies: none; coordinate verification changes with S1. Implement before S2.

## S6. Share frontend state classification and repeated CLI arguments

Problem:

The CLI's `state_summary` and TUI's `state_label` independently interpret outcome,
currency, and integration. Repeated Clap definitions also duplicate notes,
author, and description argument rules.

Primary code: `src/model.rs`, `src/main.rs`, `../linka-tui/src/app.rs`,
and any Orka frontend that independently interprets Linka state.

Implementation:

- [x] Add a derived workability/classification method on `NodeState` that
  centralizes presentation precedence. Keep outcome, currency, integration,
  reasons, and blockers accessible as independent information.
- [x] Reconcile any disagreement among current helpers, displays, and the
  design truth table before adopting it. Document any necessary correctness
  fix separately from formatting changes, especially stale pending candidates
  and terminal rejected/abandoned verifications.
- [x] Reuse the classification in frontends while allowing concise TUI labels,
  detailed CLI explanations, different colors, and first-reason formatting.
- [x] Do not persist the derived classification or introduce a second source
  of graph state. Do not put terminal styling in the graph model.
- [x] Extract `clap::Args` groups for repeated notes/notes-file, author, and
  description/file inputs where their rules actually match. Preserve defaults,
  mutual exclusions, requiredness, flags, and command-specific exceptions.
- [x] Remove duplicate semantic branches and argument definitions; update help
  text only where needed to explain existing behavior.

Acceptance criteria:

- Frontends agree on complete, ready, blocked, and awaiting integration while
  retaining review conclusions and stale/failed history in their presentation.
- Human wording can differ without duplicating graph decision rules.
- Existing valid CLI invocations remain valid and invalid combinations remain
  rejected. No stored schema or result-version changes occur.

Validation:

- [x] Use a table of meaningful states: current/stale success, failure, pending
  and accepted candidates, published/rejected candidates, blockers, and every
  verification conclusion. Confirm the chosen precedence and frontend agreement.
- [x] Test representative argument combinations and existing parser regressions;
  avoid brittle tests of every help-text line or cosmetic label.
- [x] Run Linka and Linka TUI tests and checks for other touched frontends.

Dependencies: none; consume S4's view when available without requiring it.

## S7. Replace unused capability-trait layering with one VCS seam

Problem:

`Vcs` combines `StoreHistory`, `ArtifactStore`, `ContextIdentity`,
`RepositoryIdentity`, and `BranchStore`, while current graph operations consume
the combined trait. The split adds public names, imports, implementation blocks,
and a blanket implementation without currently narrowing operation requirements.

Primary code: `src/vcs.rs`, `src/git.rs`, `src/lib.rs`, and trait imports in
Linka, Orka, and their tests/frontends.

Implementation:

- [x] Confirm all trait bounds, trait-object uses, implementations, and method
  imports across the workspace; include generic bounds, not only `dyn` uses.
- [x] Move the existing method contracts onto one object-safe `Vcs` trait.
  Retain documentation grouped by responsibility within that trait.
- [x] Implement it directly for `GitVcs` and `FakeVcs`, removing the marker
  trait, capability traits, blanket implementation, and obsolete re-exports.
- [x] Migrate caller imports and any test adapters. Document this as a Rust API
  change; keep compatibility aliases only if an actual supported caller needs
  them, with an explicit removal plan.
- [x] Preserve all method behavior, the project/workbench repository split,
  execution-context wiring, and fake-backend error injection.
- [x] Update architecture documentation to explain the single injectable seam.
  Do not remove Git publication behavior or add speculative backend support.

Acceptance criteria:

- There is one public VCS trait and one implementation per backend, with no
  capability composition layer or replacement hierarchy.
- Graph operations remain testable without invoking Git through the fake.
- Git command behavior, disk formats, locking, and publication semantics are
  unchanged. Report this as a small abstraction cleanup, not a major size cut.

Validation:

- [x] Compile and test the workspace consumers to catch trait-method resolution
  changes; run Linka and Orka tests and relevant clippy checks.
- [x] Reuse existing fake-backend and real-Git tests for capture, drift, retention,
  store history, context identity, and compare-and-swap publication. Add tests
  only if migration exposes an uncovered behavior; do not mirror trait layout.

Dependencies: none. Avoid overlapping edits to the same submission imports
while S5 is being implemented.
