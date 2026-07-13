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
- [x] Provide temporary compatibility for legacy empty repository fields, with
  a warning or schema migration.

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
- [x] Provide a compatibility reader or migration for existing logs.
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

## 15. Version and migrate the stored schema

- [x] Introduce explicit supported schema versions for definitions, results,
  snapshots, and any observation records.
- [x] Make readers accept the old and new schemas during migration.
- [x] Make writers emit only the new schema.
- [x] Add `linka migrate --check` to preview deterministic changes.
- [x] Add `linka migrate` to apply them in one explicit store commit.
- [x] Interpret legacy empty artifact repository fields through the current
  pairing during the compatibility window.
- [x] Provide compatibility projections for old `Status` and `complete` APIs
  until Orka and other consumers migrate.
- [x] Remove compatibility APIs only after all in-repository consumers use the
  new interfaces.

Acceptance criteria:

- Existing stores can be opened and migrated without losing history or facts.
- Migration is deterministic, reviewable, and idempotent.

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
