# Orka direct-Linka simplification tasks

Implementation plan for making Orka explicitly and directly orchestrate a
Linka store while preserving ownership boundaries between the applications.

This plan supersedes the generic `WorkGraph` direction in `TASKS.md`. The old
document remains as implementation history.

Status: implemented by commits `c0d6892` through `611d829`, and audited after
implementation. Checked items describe the landed design. Remaining audit
follow-ups are tracked here explicitly.

## Post-implementation audit follow-ups

- [x] Require a clean execution worktree for successful graph-only submissions;
      an empty output list must not allow undeclared changes to be omitted.
- [ ] Make prompt prose reads version-consistent with the persisted
      `WorkSnapshot`, or document and test the conflict behavior if Linka state
      changes between snapshotting and prose collection.
- [x] Explicitly retain candidate branches for accepted, failed, stale, and
      otherwise sealed attempts so their work remains available for future
      inspection and recovery.
- [ ] Add an explicit, dry-run-capable pruning policy before any attempt record
      or candidate branch is deleted.
- [ ] Expand concrete conflict coverage for lineage, context, project,
      previous-result, and readiness conflicts.

## Target design

Orka is specifically a Linka orchestrator. It uses Linka's public Rust API and
domain types directly rather than maintaining a backend-neutral graph port and
a duplicate graph model.

The dependency direction remains one-way:

```text
Orka  ──depends on──>  Linka
Linka ──does not depend on──> Orka
```

Ownership remains separate:

- Linka owns graph definitions, readiness, staleness, work snapshots, result
  validation, graph mutations, and project/output provenance.
- Orka owns agent selection policy, execution policy, prompts, durable
  attempts, transcripts, outcome interpretation, recovery, and cleanup.
- Orka may call Linka public operations but never read or mutate Linka's
  on-disk representation directly.
- Linka stores only namespaced producer evidence about Orka; it never
  interprets attempts, agents, executors, or recovery state.
- `.linka/` and `.orka/` remain separately owned stores.
- Orka retains `orka/attempts/<attempt-id>` candidate branches after removing
  their temporary worktrees. A retained branch is part of the attempt's
  inspectable evidence, including when Linka rejected the submission as stale.
  Retention is deliberate; deletion requires a future explicit pruning policy.

Keep abstraction boundaries only where implementations are genuinely
replaceable:

- `IsolatedExecutor`
- `WorkspaceManager`

Keep `FsAttemptStore` concrete until a second attempt-store implementation is
actually required.

## Phase 0 — Confirm the Linka protocol

- [x] Treat `linka::WorkSnapshot` as the authoritative frozen work input.
- [x] Treat `linka::ResultSubmission` and `linka::ops::submit_result` as the
      authoritative version-checked result protocol.
- [x] Audit `WorkSnapshot` and confirm that it freezes every fact required by
      Orka:
  - [x] node identity;
  - [x] definition version;
  - [x] dependency pins and outcomes;
  - [x] lineage pins and outcomes;
  - [x] explicit context pins;
  - [x] project repository, revision, and tree identity;
  - [x] previous result version.
- [x] Confirm that `submit_result` revalidates every frozen field and returns
      structured `SubmissionConflict` values without changing the store on a
      conflict.
- [x] Make `SubmissionConflict` serializable if Orka needs to persist it in a
      seal record. Prefer persisting structured conflicts over formatted
      strings.
- [x] Decide whether deprecated orchestration helpers (`ExpectedInput`,
      `CheckedCompletion`, `complete_checked`, and `verify_frozen`) remain for
      other callers or are removed after Orka migrates.

Exit criterion: Linka has one documented snapshot/submission protocol that is
sufficient for Orka; Orka will not need to decompose and reconstruct version
tokens.

## Phase 1 — Complete Linka's capture-and-submit API

Orka performs work in an attempt-specific Git worktree. Linka must remain the
owner of output validation, capture, artifact construction, and result
submission semantics.

- [x] Add a public Linka operation that accepts an existing `WorkSnapshot` and
      submits work performed in the supplied `Vcs` execution context.
- [x] Prefer a single operation with semantics equivalent to:

  ```rust
  pub fn capture_submission(
      store: &Store,
      vcs: &dyn Vcs,
      snapshot: WorkSnapshot,
      outputs: &[ProjectPath],
      message: Option<String>,
      outcome: Outcome,
      notes: String,
      author: Author,
      producer: Option<ProducerEvidence>,
  ) -> Result<SubmissionResult, SubmissionError>;
  ```

  The exact name and ownership form may differ, but the operation must consume
  the caller's frozen snapshot rather than taking a fresh one.

- [x] Make the operation enforce Linka's existing rules for:
  - [x] normalized and valid project-relative paths;
  - [x] duplicate and overlapping declared paths;
  - [x] undeclared or dirty workspace changes;
  - [x] commit-message construction;
  - [x] output commit capture;
  - [x] artifact repository identity;
  - [x] retained output references;
  - [x] snapshot revalidation;
  - [x] graph result persistence.
- [x] Define capture/conflict ordering explicitly. A graph conflict must never
      record a result. If an output commit is captured before conflict
      detection, ensure that retaining or cleaning the unsubmitted commit is
      safe and documented.
- [x] Support `Outcome::Done` with no declared outputs without requiring a
      project commit.
- [x] Support `Outcome::Failed` against the original `WorkSnapshot`, with no
      output artifact.
- [x] Do not use `ops::fail` from Orka: it observes current inputs and therefore
      cannot faithfully record what a completed attempt ran against.
- [x] Return conflicts as `SubmissionError::Conflict(Vec<SubmissionConflict>)`;
      reserve other errors for evaluation, storage, Git, or invariant failures.
- [x] Add Linka tests covering:
  - [x] successful submission with captured files;
  - [x] successful graph-only submission;
  - [x] failure submission using the frozen snapshot;
  - [x] definition conflict;
  - [x] dependency conflict;
  - [x] lineage conflict;
  - [x] context conflict;
  - [x] project conflict;
  - [x] previous-result conflict;
  - [x] readiness conflict;
  - [x] no graph mutation on every conflict;
  - [x] producer evidence preservation.

Exit criterion: an external caller can snapshot work, operate in another
worktree, and submit success or failure against that exact snapshot without
reimplementing Linka capture or validation logic.

## Phase 2 — Introduce Orka's durable attempt input

Replace Orka's duplicate frozen graph representation with Linka's snapshot,
while retaining the prompt material that Orka itself owns.

- [x] Add an Orka-owned `AttemptInput` similar to:

  ```rust
  #[derive(Clone, Debug, Serialize, Deserialize)]
  pub struct AttemptInput {
      pub snapshot: linka::WorkSnapshot,
      pub description: String,
      pub dependency_context: Vec<DependencyContext>,
  }

  #[derive(Clone, Debug, Serialize, Deserialize)]
  pub struct DependencyContext {
      pub node: linka::NodeId,
      pub title: String,
      pub result_notes: String,
  }
  ```

- [x] Decide whether lineage context is presented separately or included in a
      generalized prompt-context collection. Whatever the choice, preserve the
      exact prose supplied to the agent in the attempt record.
- [x] Use `snapshot.project.revision` as the workspace's starting commit.
- [x] Remove the separate `input_commit` copy once all call sites and stored
      records use the snapshot field.
- [x] Change prompt construction to consume `AttemptInput`.
- [x] Keep prompt prose distinct from Linka's validation token: prompt fields
      are frozen audit material, while `WorkSnapshot` is authoritative for
      submission.
- [x] Change `AttemptRecord.frozen` to `AttemptRecord.input` (or another name
      that does not imply an alternate graph model).
- [x] Bump the attempt schema from 1 to 2.
- [x] Add round-trip tests for every Linka type embedded in the attempt record.

### Existing-attempt policy

- [x] Determine whether schema-1 attempts exist outside tests.
- [x] If compatibility is required:
  - [x] define `AttemptRecordV1` matching the old `FrozenInput` layout;
  - [x] define `AttemptRecordV2` containing `AttemptInput`;
  - [x] dispatch loading by the explicit schema number;
  - [x] continue to support listing, showing, sealing, and workspace cleanup
        for V1 attempts;
  - [x] do not reconstruct a `WorkSnapshot` from V1 fingerprints because the
        old record omits lineage, context, repository identity, and previous
        result state;
  - [x] refuse automatic submission of unfinished V1 attempts with a clear
        migration explanation;
  - [x] either retain V1 attempts for intervention or seal them interrupted
        through an explicit migration command/policy.
- [x] If compatibility is not required, document the intentional schema break
      and remove test data before merging.

Exit criterion: every new attempt durably stores Linka's exact `WorkSnapshot`
before any workspace or execution side effect.

## Phase 3 — Replace the graph adapter with concrete Linka integration

Add an organizational boundary for Linka-specific orchestration operations,
but do not define a graph trait or pretend that other graph backends are
supported.

- [x] Add `orka/src/linka_work.rs` (name may be adjusted) with a concrete
      `LinkaWork` type borrowing a `linka::Store`.
- [x] Do not introduce a `WorkGraph`-like trait for this type.
- [x] Give `LinkaWork` the minimum orchestration-facing operations:
  - [x] list Linka-ready, machine-assignable work;
  - [x] prepare and return an `AttemptInput`;
  - [x] submit a successful attempt;
  - [x] submit a failed attempt.
- [x] Use `linka::ops::ready_nodes(..., Some(Author::Machine))` for selection.
      Orka chooses among Linka-ready results but does not derive readiness.
- [x] Return `linka::NodeId` from selection rather than wrapping it.
- [x] During attempt preparation:
  - [x] ask Linka to validate and snapshot the selected node;
  - [x] read the selected description through `linka::Store` public methods;
  - [x] load dependency and lineage prose needed for the prompt;
  - [x] return the snapshot and frozen prose as one `AttemptInput`.
- [x] Define a small Orka `ReadyWork` view only if the CLI needs a title beside
      the Linka node ID.
- [x] Define an Orka `AgentOutcome` for the interpreted agent declaration. It
      may contain success outputs/message/notes or failure notes, but it must
      not duplicate Linka snapshots or graph versions.
- [x] Convert declared output strings into `linka::ProjectPath` before capture.
      Report invalid paths as contract/policy failures rather than passing raw
      strings into Git operations.
- [x] Construct `linka::ResultSubmission` or call the Phase-1 combined
      capture/submission operation with the exact persisted snapshot.
- [x] Use `linka::Author::Machine` for Orka-produced results.
- [x] Map `SubmissionError::Conflict` to Orka's stale-at-submit terminal state.
- [x] Propagate evaluation/storage/invariant errors as operational errors; do
      not misclassify them as ordinary staleness.

Exit criterion: this concrete module covers every graph operation used by the
engine and contains no mirrored Linka fingerprints or pins.

## Phase 4 — Record Orka producer evidence in Linka

- [x] Build `linka::ProducerEvidence` for every submitted agent outcome.
- [x] Use the stable namespace `orka`.
- [x] Include at least:
  - [x] attempt ID;
  - [x] execution backend;
  - [x] backend reference, when present;
  - [x] observed start and finish timestamps;
  - [x] observed exit code.
- [x] Decide whether to include the configured command or a command digest.
      Avoid duplicating the full request if the attempt ID is sufficient to
      locate it in `.orka/`.
- [x] Never put the transcript or mutable filesystem paths into Linka result
      metadata.
- [x] Treat the executor report as authoritative; do not accept backend/model
      evidence asserted by the agent.
- [x] Add tests proving Linka preserves but does not interpret the namespaced
      evidence.

Exit criterion: a Linka result identifies the Orka attempt that produced it.
Detailed execution records remain Orka-owned; evidence for a produced output is
also retained through Linka's generic opaque node attachments.

## Phase 5 — Refactor the engine to use Linka directly

- [x] Replace `Engine.graph: &dyn WorkGraph` with either:

  ```rust
  pub linka: LinkaWork<'a>
  ```

  or a direct `&linka::Store` if that produces clearer ownership. Prefer the
  concrete `LinkaWork` value when it keeps Linka calls out of lifecycle code.

- [x] Change `Engine::run_node` to accept `&linka::NodeId`.
- [x] Change `RunReport.node` and `RecoveryReport.node` to `linka::NodeId`.
- [x] Change `run_next` to select through `LinkaWork::ready_for_machine`.
- [x] Change attempt creation to persist `AttemptInput`.
- [x] Plan and prepare the workspace from
      `input.snapshot.project.revision`.
- [x] Change execution environment construction to publish the Linka node ID
      directly as `ORKA_NODE`.
- [x] Change prompt generation to use the frozen prose in `AttemptInput`.
- [x] Change `settle` to:
  - [x] read and interpret the Orka outcome declaration;
  - [x] validate the presence of a success workspace;
  - [x] submit through concrete Linka integration;
  - [x] include the captured execution report as producer evidence;
  - [x] seal accepted success, accepted failure, or submission conflict;
  - [x] leave operational failures unsealed and recoverable where safe.
- [x] Pass the full `ExecutionReport` into settlement rather than only its exit
      code, so producer evidence is available during normal execution and
      recovery.
- [x] Preserve current handling of a declared outcome plus a nonzero exit: the
      outcome is handled and backend failure remains separately reportable.
- [x] Preserve conservative recovery:
  - [x] never invent an outcome without exit evidence;
  - [x] resubmit executed-but-unsealed attempts using the persisted snapshot;
  - [x] keep submission idempotent or classify an already-recorded result
        deterministically;
  - [x] never discard a dirty workspace;
  - [x] clean only sealed attempts or attempts that cannot have a result.
- [x] Review crash windows around output capture, Linka submission, Orka seal,
      and workspace cleanup. Add a durable submission marker if the Linka API
      cannot make recovery unambiguous from snapshot/result provenance.

Exit criterion: the engine contains no generic graph port and does not
translate between Orka and Linka graph models.

## Phase 6 — Keep the agent outcome contract Orka-owned

- [x] Retain `DeclaredOutcome`, `DeclaredKind`, and the declaration/exit failure
      matrix in Orka.
- [x] Rename the old graph-oriented `WorkOutcome` to an Orka-owned
      `AgentOutcome` or fold it into `Decision`.
- [x] Do not deserialize agent-written TOML into `linka::ResultSubmission`.
- [x] Keep these decisions in Orka:
  - [x] missing declaration plus zero exit is a contract violation;
  - [x] missing declaration plus nonzero exit is interrupted;
  - [x] declared success or failure is eligible for Linka submission;
  - [x] a nonzero exit accompanying a declaration is reported separately.
- [x] Let the Linka integration module perform the only translation from
      `AgentOutcome` to Linka result operations.
- [x] Validate that failure declarations cannot claim outputs.
- [x] Define how an empty success output list differs from a graph-only answer,
      if Linka makes that distinction.

Exit criterion: agents declare an Orka execution outcome; only trusted Orka
code constructs Linka mutations.

## Phase 7 — Remove the duplicate graph layer

- [x] Remove `WorkGraph` from `ports.rs`.
- [x] Delete `orka/src/linka_graph.rs`.
- [x] Delete `FakeWorkGraph`.
- [x] Remove the `linka_graph` module export from `lib.rs`.
- [x] Remove Orka's duplicate graph types:
  - [x] `NodeId`;
  - [x] `WorkItem` where replaced by a concrete ready-work view;
  - [x] `DefinitionFingerprint`;
  - [x] `ResultFingerprint`;
  - [x] `ArtifactPin`;
  - [x] `FrozenDependency`;
  - [x] `FrozenInput`;
  - [x] graph-oriented `WorkOutcome`;
  - [x] `Submission`;
  - [x] `SubmitOutcome`.
- [x] Replace every remaining call site with Linka types or narrowly scoped
      Orka execution types.
- [x] Remove conversion helpers such as `definition_fingerprint` and
      `expected_input`.
- [x] Remove fake/real `WorkGraph` contract tests.
- [x] Split or rename `ports.rs` after graph types are gone. Prefer focused
      modules such as `executor.rs` and `workspace.rs` if that is clearer.
- [x] Search the Orka source for removed type names and generic graph-backend
      claims; require no matches except migration documentation.

Exit criterion: the only graph domain model visible in Orka is Linka's public
model.

## Phase 8 — Simplify CLI and workbench construction

- [x] Replace `Workbench::graph()` with `Workbench::linka_store()`.
- [x] Open `.linka/` using `linka::Store::open`.
- [x] Build `GitWorkspaces` from `store.project_root()`.
- [x] Parse CLI node arguments immediately as `linka::NodeId` and report
      Linka's validation error at the command boundary.
- [x] Implement `orka ready` through concrete Linka selection.
- [x] Update `orka attempts`, `orka show`, and `orka recover` for the new
      attempt schema and Linka node IDs.
- [x] Keep workbench discovery based on the nearest `.linka/`; Orka is now
      explicitly a Linka-workbench application.
- [x] Update CLI descriptions to say that Orka orchestrates isolated agent
      attempts for work in a Linka store.
- [x] Ensure Orka configuration continues to decide agent command, mounts,
      network policy, and executor backend; none of these belong in Linka.

Exit criterion: CLI construction contains no adapter setup and makes Orka's
direct Linka dependency obvious.

## Phase 9 — Restructure tests

### Pure Orka tests

- [x] Keep fast tests for:
  - [x] attempt phase derivation;
  - [x] atomic record writing and schema loading;
  - [x] prompt construction from `AttemptInput`;
  - [x] outcome declaration parsing;
  - [x] declaration/exit decision matrix;
  - [x] execution request construction;
  - [x] workspace cleanup policy;
  - [x] seal idempotency;
  - [x] recovery classification that does not require graph mutation.
- [x] Continue using `FakeExecutor` and `FakeWorkspaces` for these tests.
- [x] Extract pure lifecycle helpers where necessary so graph fakes are not
      reintroduced merely to keep unit tests small.

### Concrete Linka integration tests

- [x] Add a reusable fixture containing:
  - [x] a temporary Linka workbench/store;
  - [x] a temporary project Git repository;
  - [x] valid Git identity configuration;
  - [x] helpers to add, edit, link, complete, and fail nodes;
  - [x] concrete `LinkaWork` construction.
- [x] Test machine-only selection and ordering.
- [x] Test snapshot and prompt context preparation.
- [x] Test dependency and lineage handling.
- [x] Test success with and without project outputs.
- [x] Test failure evidence against the frozen snapshot.
- [x] Test every structured Linka conflict.
- [x] Test producer evidence.

### End-to-end Orka tests

- [x] Keep a smaller suite using a real Linka store, real Git workspaces,
      `FsAttemptStore`, and `FakeExecutor`.
- [x] Cover the full successful lifecycle.
- [x] Cover graph-only success.
- [x] Cover declared failure.
- [x] Cover graph mutation during execution and stale-at-submit sealing.
- [x] Cover output capture and publication expectations.
- [x] Cover a nonzero backend exit with a declared outcome.
- [x] Cover recovery from every durable attempt phase.
- [x] Cover the crash window after Linka accepts a result but before Orka seals
      the attempt.
- [x] Prove a second recovery pass performs no duplicate mutation.

Exit criterion: tests exercise actual Linka semantics where those semantics
matter, while execution and recovery policy remain cheaply unit-testable.

## Phase 10 — Documentation and architectural checks

- [x] Update `orka/DESIGN.md`:
  - [x] describe Orka as specifically orchestrating Linka work;
  - [x] replace the `WorkGraph` boundary with direct use of Linka's public
        snapshot/submission API;
  - [x] retain executor and workspace boundaries;
  - [x] explicitly reject a generic graph-backend goal;
  - [x] document `.linka/` and `.orka/` ownership;
  - [x] document producer evidence and recovery responsibilities.
- [x] Update `designs/SEPARATE_APPLICATIONS.md` to say that Orka depends on
      Linka's public library API and value types, rather than an Orka-owned
      generic application interface.
- [x] Update `orka/README.md`, examples, and CLI help.
- [x] Mark the old generic-port tasks in `orka/TASKS.md` as historical or link
      from them to this plan.
- [x] Ensure Linka documentation describes `WorkSnapshot` and result submission
      as a stable public protocol for orchestrators and other callers.
- [x] Add or retain dependency checks proving:
  - [x] Linka builds and tests without Orka;
  - [x] Linka does not import Orka types;
  - [x] Orka never accesses Linka storage files except through Linka APIs;
  - [x] Orka's executor and workspace policy does not move into Linka.

Exit criterion: documentation describes the architecture the code actually
implements, without backend-neutral language that the design no longer
supports.

## Final verification

- [x] Format all changed Rust and Markdown where applicable.
- [x] Run the Linka unit and integration test suite.
- [x] Run the Orka unit and integration test suite.
- [x] Run end-to-end Orka tests with a real Linka store and Git worktree.
- [x] Run recovery tests for every recorded phase and relevant crash window.
- [x] Build Linka independently of Orka.
- [x] Build the full workspace.
- [x] Verify searches for `WorkGraph`, `FakeWorkGraph`, `FrozenInput`,
      `DefinitionFingerprint`, and `linka_graph` find no live Orka code.
- [x] Verify Orka contains no direct reads or writes beneath `.linka/`.
- [x] Verify Linka contains no knowledge of `.orka/`, attempt phases, prompts,
      transcripts, executors, or agent outcome files.

## Completion criteria

The simplification is complete when:

1. Orka directly opens and orchestrates a `linka::Store`.
2. Orka persists `linka::WorkSnapshot` as the authoritative input to every new
   attempt.
3. Success and failure are submitted against that exact snapshot through
   Linka's public, version-checked API.
4. Orka has no generic graph port or duplicate Linka version model.
5. Linka remains independently usable and owns all graph semantics.
6. Orka continues to own attempts, execution policy, transcripts, recovery,
   and cleanup.
7. Executor and workspace substitutions remain possible without weakening the
   direct Linka relationship.
8. Crash recovery cannot silently complete stale work or duplicate an accepted
   Linka result.
