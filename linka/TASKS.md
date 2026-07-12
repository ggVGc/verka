# Linka correctness and scope tasks

This backlog brings Linka's implementation into line with `DESIGN.md`: Linka
owns versioned graph facts and their derived state, while execution, worktree,
retry, review, and publication policy belong to applications such as Orka.

Tasks are ordered by dependency. Semantic correctness comes before API cleanup
and migration.

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

- [ ] Change readiness so a successful-but-stale node is ready when its current
  `depends_on` dependencies are complete.
- [ ] Keep a stale node blocked when any required dependency is incomplete.
- [ ] Make dependents of stale nodes remain blocked.
- [ ] Present the previous stale result as evidence without treating it as a
  current completion.

Tests:

- [ ] Changed definition makes a previously successful node stale and ready.
- [ ] Changed dependency result/output makes its consumer stale and ready once
  dependencies are complete.
- [ ] Changed context and drifted output make a node stale and ready.
- [ ] A stale node with an incomplete dependency is blocked.
- [ ] A dependent of a stale node is blocked.
- [ ] A failed node is ready only when its dependencies are complete.

## 4. Stop converting failures into graph facts

- [ ] Change derived queries to return `Result`.
- [ ] Distinguish legitimate absence, staleness, corruption, and backend
  failure.
- [ ] Treat a missing context file as staleness, but a permission or I/O error
  as an error.
- [ ] Treat a proven-missing artifact as staleness, but an artifact-backend
  failure as an error.
- [ ] Update list-oriented CLI commands to report per-node errors and exit
  nonzero if any node could not be evaluated.
- [ ] Ensure `stale`, `ready`, and `blocked` never print a clean result after
  silently skipping errors.

Tests:

- [ ] Malformed definitions and results are not reported as open or ready.
- [ ] Context read failures are not reported as missing files.
- [ ] Artifact lookup failures are not reported as drift or absence.
- [ ] A missing target node is distinguished from an unreadable target node.

## 5. Add validated graph identifiers and project paths

- [ ] Introduce `NodeId` and `ProjectPath` newtypes.
- [ ] Reject empty, absolute, traversal, control-character, and platform-prefix
  forms.
- [ ] Define whether `.git` paths are always forbidden; default to forbidding
  them.
- [ ] Normalize portable path separators before persistence.
- [ ] Validate values at CLI parsing, public library entry points,
  deserialization, and store directory discovery.
- [ ] Prevent symlink-based escape when reading working-tree files, or avoid it
  by resolving context through repository objects.

Tests:

- [ ] Reject `..`, `../secret`, absolute paths, backslash traversal, `.git`
  internals, and symlinks escaping the project root.
- [ ] Accept valid node IDs and nested project-relative paths.

## 6. Expand `linka check` into semantic fsck

- [ ] Validate supported definition and result schema versions.
- [ ] Validate node IDs, required files, paired result files, and normalized
  paths.
- [ ] Retain edge checks for missing targets, duplicates, self-links, and
  `depends_on` cycles.
- [ ] Validate unique consumed-node and context pins.
- [ ] Validate that result pins correspond to declared relationship edges.
- [ ] Require successful `depends_on` pins to contain successful result
  evidence.
- [ ] Define and validate the weaker `derived_from` pin invariant.
- [ ] Validate supported artifact schemes and repository identities.
- [ ] Keep historical pin mismatches out of fsck: those are staleness, not
  corruption.
- [ ] Add an artifact-aware check mode that verifies referenced commits and
  retained output refs.
- [ ] Keep `check` read-only; make any future repair operations explicit.

Acceptance criteria:

- Hand edits and merges cannot create parseable but semantically impossible
  results without `linka check` reporting them.
- Structural checking works without a project checkout; artifact checking is
  explicitly opt-in.

## 7. Add frozen work snapshots

- [ ] Add a graph-owned `WorkSnapshot` containing the node ID, definition
  version, dependency/lineage pins, and explicit context pins.
- [ ] Represent the project input revision as a generic artifact/project
  snapshot, separate from graph identity.
- [ ] Add a `snapshot_work` operation that rejects unknown, blocked, corrupt,
  or unreadable nodes.
- [ ] Permit snapshotting a stale node when its current dependencies are
  complete.
- [ ] Keep attempt IDs, sessions, branches, worktree paths, and backend details
  out of snapshot types.

Tests:

- [ ] Snapshots contain exact definition, dependency result/output, lineage,
  context, and project revision identities.
- [ ] Blocked and corrupt nodes cannot be snapshotted.

## 8. Add compare-and-record result submission

- [ ] Add `ResultSubmission`, carrying a frozen snapshot, outcome, optional
  output artifact, notes, author, and optional opaque producer evidence.
- [ ] Add `submit_result`, which rechecks every frozen graph and context version
  immediately before writing.
- [ ] Return structured conflicts for definition, dependency, context, and
  readiness changes.
- [ ] Include the expected previous result version to prevent concurrent
  overwrites.
- [ ] Perform snapshot revalidation and result replacement under a store
  mutation lock.
- [ ] Reject a conflicting submission without overwriting the prior result.
- [ ] Keep `complete` temporarily as a short-lived convenience wrapper that
  snapshots, captures, and submits within one call.
- [ ] Document that long-running workers must use the explicit snapshot and
  submission operations.

Tests:

- [ ] Reject submission after changes to definition metadata or description.
- [ ] Reject submission after dependency definition, result, or output changes.
- [ ] Reject submission after context changes or a new required dependency is
  added.
- [ ] Reject concurrent submission when the expected previous result changed.
- [ ] Preserve the previous result on every rejection.

## 9. Enforce valid successful results

- [ ] Require a successful submission to match its snapshot and have all
  current `depends_on` nodes complete.
- [ ] Forbid a successful `depends_on` pin with no result or with failed/stale
  evidence.
- [ ] Separate required dependency pins from weaker lineage pins if that makes
  their invariants explicit.
- [ ] Define `derived_from` behavior: it does not block readiness, but observed
  lineage versions participate in provenance and later staleness.
- [ ] Make `respond` an output-free convenience over ordinary checked result
  submission rather than a special graph concept.
- [ ] Continue allowing failed evidence to be recorded without falsely marking
  the node complete.

Tests:

- [ ] A blocked node cannot receive a successful result.
- [ ] Failed, stale, and result-less required dependencies prevent success.
- [ ] Output-free success follows the same validation as output-producing work.
- [ ] Lineage does not accidentally become a scheduling dependency.

## 10. Make context and artifact identity revision-based

- [ ] Record the source project revision/tree on each context snapshot or on
  the containing work snapshot.
- [ ] Check context through an explicit repository revision rather than the
  process's currently checked-out worktree.
- [ ] Define currency relative to an explicit current project revision supplied
  by the caller.
- [ ] Ensure dirty working-tree state does not silently redefine graph
  currency.
- [ ] Populate `ArtifactRef.repository` from the verified project pairing rather
  than leaving it empty.
- [ ] Reject artifacts from a different paired repository.
- [ ] Provide temporary compatibility for legacy empty repository fields, with
  a warning or schema migration.

Tests:

- [ ] Context captured in a linked worktree is checked correctly when the main
  checkout differs.
- [ ] Identical blobs remain current across revisions.
- [ ] Missing paths at the comparison revision become stale.
- [ ] Artifacts from another repository are rejected.

## 11. Narrow Linka's VCS and storage interfaces

- [ ] Split the broad `Vcs` trait into narrow graph-facing capabilities for
  store history, artifact inspection/retention, and context identity.
- [ ] Remove branch, revision-resolution, worktree, and publication methods
  from Linka's public traits.
- [ ] Move `Worktree`, candidate-branch helpers, worktree cleanup, ref
  publication, and their tests into Orka or a project adapter owned by Orka.
- [ ] Keep only generic artifact facts and inspection in Linka.
- [ ] Add dependency/architecture checks ensuring Linka does not import Orka,
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

- [ ] Move `work.jsonl`, `read_work_log`, `open_work_log`, and
  `commit_work_log` to Orka's attempt/session storage.
- [ ] Provide a compatibility reader or migration for existing logs.
- [ ] Replace transcript-specific `amend_context` with a neutral context
  observation input, if Linka needs such an operation at all.
- [ ] Prefer immutable observation records keyed by result version over
  rewriting an existing result after completion.
- [ ] Make Orka responsible for mining transcripts and submitting observations.
- [ ] Confirm observations cannot change a definition or silently replace the
  result they refer to.

Acceptance criteria:

- Linka neither knows nor cares that context observations came from a session
  transcript.
- Interaction logs do not participate in Linka graph versions or derived state.

## 13. Add recoverable cross-repository completion

- [ ] Put a submission journal in the coordinating application layer (Orka or
  the human CLI workflow), not in Linka's graph semantics.
- [ ] Record a submission ID, node, frozen snapshot, intended result, output
  artifact, and phase.
- [ ] Define phases such as prepared, artifact retained, result written, store
  committed, and finalized.
- [ ] Make every phase idempotent.
- [ ] Add recovery that distinguishes finalized, already finalized, conflict,
  artifact-only, store-write-pending, and corrupt states.
- [ ] Ensure recovery rechecks the expected result and graph snapshot before
  finalization.
- [ ] Do not silently discard an output artifact when graph submission fails.

Tests:

- [ ] Inject a failure before and after every phase and recover successfully.
- [ ] Run recovery repeatedly and get the same final state.
- [ ] Handle two submissions from the same snapshot without overwriting the
  winner.
- [ ] Preserve recoverable evidence when graph state changes before submission.

## 14. Update CLI state presentation

- [ ] Show complete, ready, blocked, stale prior result, failed prior attempt,
  and corruption as distinct conditions.
- [ ] Include concise structured reasons for readiness, blocking, and
  staleness.
- [ ] Keep `stale` as a historical-result query even though stale nodes also
  appear in `ready` or `blocked`.
- [ ] Ensure all commands use the authoritative `NodeState` derivation.
- [ ] Define stable nonzero exit behavior for corruption, backend failure,
  conflicts, failed checks, and unsettled nodes.

Example output shape:

```text
node-...  complete
node-...  ready (previous result stale: dependency A changed)
node-...  ready (previous attempt failed)
node-...  blocked by A: stale
node-...  error: malformed result.toml
```

## 15. Version and migrate the stored schema

- [ ] Introduce explicit supported schema versions for definitions, results,
  snapshots, and any observation records.
- [ ] Make readers accept the old and new schemas during migration.
- [ ] Make writers emit only the new schema.
- [ ] Add `linka migrate --check` to preview deterministic changes.
- [ ] Add `linka migrate` to apply them in one explicit store commit.
- [ ] Interpret legacy empty artifact repository fields through the current
  pairing during the compatibility window.
- [ ] Provide compatibility projections for old `Status` and `complete` APIs
  until Orka and other consumers migrate.
- [ ] Remove compatibility APIs only after all in-repository consumers use the
  new interfaces.

Acceptance criteria:

- Existing stores can be opened and migrated without losing history or facts.
- Migration is deterministic, reviewable, and idempotent.

## Final verification

- [ ] Add an end-to-end test covering: create graph, snapshot ready work,
  change an input, reject stale submission, resnapshot, submit successfully,
  make a dependency change, and select the consumer for rework.
- [ ] Add an end-to-end crash-recovery test spanning artifact capture and graph
  submission.
- [ ] Run `cargo test` and clippy for Linka and every in-repository consumer.
- [ ] Verify Linka's public API contains no attempt, session, worktree, review,
  retry, or publication policy.
- [ ] Verify every statement in `linka/DESIGN.md` has a corresponding test or
  documented external responsibility.
