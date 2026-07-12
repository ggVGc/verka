# Orka direct-Linka simplification tasks

Implementation plan for making Orka explicitly and directly orchestrate a
Linka store while preserving ownership boundaries between the applications.

This plan supersedes the generic `WorkGraph` direction in `TASKS.md`. The old
document remains as implementation history.

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

Keep abstraction boundaries only where implementations are genuinely
replaceable:

- `IsolatedExecutor`
- `WorkspaceManager`

Keep `FsAttemptStore` concrete until a second attempt-store implementation is
actually required.

## Phase 0 — Confirm the Linka protocol

- [ ] Treat `linka::WorkSnapshot` as the authoritative frozen work input.
- [ ] Treat `linka::ResultSubmission` and `linka::ops::submit_result` as the
      authoritative version-checked result protocol.
- [ ] Audit `WorkSnapshot` and confirm that it freezes every fact required by
      Orka:
  - [ ] node identity;
  - [ ] definition version;
  - [ ] dependency pins and outcomes;
  - [ ] lineage pins and outcomes;
  - [ ] explicit context pins;
  - [ ] project repository, revision, and tree identity;
  - [ ] previous result version.
- [ ] Confirm that `submit_result` revalidates every frozen field and returns
      structured `SubmissionConflict` values without changing the store on a
      conflict.
- [ ] Make `SubmissionConflict` serializable if Orka needs to persist it in a
      seal record. Prefer persisting structured conflicts over formatted
      strings.
- [ ] Decide whether deprecated orchestration helpers (`ExpectedInput`,
      `CheckedCompletion`, `complete_checked`, and `verify_frozen`) remain for
      other callers or are removed after Orka migrates.

Exit criterion: Linka has one documented snapshot/submission protocol that is
sufficient for Orka; Orka will not need to decompose and reconstruct version
tokens.

## Phase 1 — Complete Linka's capture-and-submit API

Orka performs work in an attempt-specific Git worktree. Linka must remain the
owner of output validation, capture, artifact construction, and result
submission semantics.

- [ ] Add a public Linka operation that accepts an existing `WorkSnapshot` and
      submits work performed in the supplied `Vcs` execution context.
- [ ] Prefer a single operation with semantics equivalent to:

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

- [ ] Make the operation enforce Linka's existing rules for:
  - [ ] normalized and valid project-relative paths;
  - [ ] duplicate and overlapping declared paths;
  - [ ] undeclared or dirty workspace changes;
  - [ ] commit-message construction;
  - [ ] output commit capture;
  - [ ] artifact repository identity;
  - [ ] retained output references;
  - [ ] snapshot revalidation;
  - [ ] graph result persistence.
- [ ] Define capture/conflict ordering explicitly. A graph conflict must never
      record a result. If an output commit is captured before conflict
      detection, ensure that retaining or cleaning the unsubmitted commit is
      safe and documented.
- [ ] Support `Outcome::Done` with no declared outputs without requiring a
      project commit.
- [ ] Support `Outcome::Failed` against the original `WorkSnapshot`, with no
      output artifact.
- [ ] Do not use `ops::fail` from Orka: it observes current inputs and therefore
      cannot faithfully record what a completed attempt ran against.
- [ ] Return conflicts as `SubmissionError::Conflict(Vec<SubmissionConflict>)`;
      reserve other errors for evaluation, storage, Git, or invariant failures.
- [ ] Add Linka tests covering:
  - [ ] successful submission with captured files;
  - [ ] successful graph-only submission;
  - [ ] failure submission using the frozen snapshot;
  - [ ] definition conflict;
  - [ ] dependency conflict;
  - [ ] lineage conflict;
  - [ ] context conflict;
  - [ ] project conflict;
  - [ ] previous-result conflict;
  - [ ] readiness conflict;
  - [ ] no graph mutation on every conflict;
  - [ ] producer evidence preservation.

Exit criterion: an external caller can snapshot work, operate in another
worktree, and submit success or failure against that exact snapshot without
reimplementing Linka capture or validation logic.

## Phase 2 — Introduce Orka's durable attempt input

Replace Orka's duplicate frozen graph representation with Linka's snapshot,
while retaining the prompt material that Orka itself owns.

- [ ] Add an Orka-owned `AttemptInput` similar to:

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

- [ ] Decide whether lineage context is presented separately or included in a
      generalized prompt-context collection. Whatever the choice, preserve the
      exact prose supplied to the agent in the attempt record.
- [ ] Use `snapshot.project.revision` as the workspace's starting commit.
- [ ] Remove the separate `input_commit` copy once all call sites and stored
      records use the snapshot field.
- [ ] Change prompt construction to consume `AttemptInput`.
- [ ] Keep prompt prose distinct from Linka's validation token: prompt fields
      are frozen audit material, while `WorkSnapshot` is authoritative for
      submission.
- [ ] Change `AttemptRecord.frozen` to `AttemptRecord.input` (or another name
      that does not imply an alternate graph model).
- [ ] Bump the attempt schema from 1 to 2.
- [ ] Add round-trip tests for every Linka type embedded in the attempt record.

### Existing-attempt policy

- [ ] Determine whether schema-1 attempts exist outside tests.
- [ ] If compatibility is required:
  - [ ] define `AttemptRecordV1` matching the old `FrozenInput` layout;
  - [ ] define `AttemptRecordV2` containing `AttemptInput`;
  - [ ] dispatch loading by the explicit schema number;
  - [ ] continue to support listing, showing, sealing, and workspace cleanup
        for V1 attempts;
  - [ ] do not reconstruct a `WorkSnapshot` from V1 fingerprints because the
        old record omits lineage, context, repository identity, and previous
        result state;
  - [ ] refuse automatic submission of unfinished V1 attempts with a clear
        migration explanation;
  - [ ] either retain V1 attempts for intervention or seal them interrupted
        through an explicit migration command/policy.
- [ ] If compatibility is not required, document the intentional schema break
      and remove test data before merging.

Exit criterion: every new attempt durably stores Linka's exact `WorkSnapshot`
before any workspace or execution side effect.

## Phase 3 — Replace the graph adapter with concrete Linka integration

Add an organizational boundary for Linka-specific orchestration operations,
but do not define a graph trait or pretend that other graph backends are
supported.

- [ ] Add `orka/src/linka_work.rs` (name may be adjusted) with a concrete
      `LinkaWork` type borrowing a `linka::Store`.
- [ ] Do not introduce a `WorkGraph`-like trait for this type.
- [ ] Give `LinkaWork` the minimum orchestration-facing operations:
  - [ ] list Linka-ready, machine-assignable work;
  - [ ] prepare and return an `AttemptInput`;
  - [ ] submit a successful attempt;
  - [ ] submit a failed attempt.
- [ ] Use `linka::ops::ready_nodes(..., Some(Author::Machine))` for selection.
      Orka chooses among Linka-ready results but does not derive readiness.
- [ ] Return `linka::NodeId` from selection rather than wrapping it.
- [ ] During attempt preparation:
  - [ ] ask Linka to validate and snapshot the selected node;
  - [ ] read the selected description through `linka::Store` public methods;
  - [ ] load dependency and lineage prose needed for the prompt;
  - [ ] return the snapshot and frozen prose as one `AttemptInput`.
- [ ] Define a small Orka `ReadyWork` view only if the CLI needs a title beside
      the Linka node ID.
- [ ] Define an Orka `AgentOutcome` for the interpreted agent declaration. It
      may contain success outputs/message/notes or failure notes, but it must
      not duplicate Linka snapshots or graph versions.
- [ ] Convert declared output strings into `linka::ProjectPath` before capture.
      Report invalid paths as contract/policy failures rather than passing raw
      strings into Git operations.
- [ ] Construct `linka::ResultSubmission` or call the Phase-1 combined
      capture/submission operation with the exact persisted snapshot.
- [ ] Use `linka::Author::Machine` for Orka-produced results.
- [ ] Map `SubmissionError::Conflict` to Orka's stale-at-submit terminal state.
- [ ] Propagate evaluation/storage/invariant errors as operational errors; do
      not misclassify them as ordinary staleness.

Exit criterion: this concrete module covers every graph operation used by the
engine and contains no mirrored Linka fingerprints or pins.

## Phase 4 — Record Orka producer evidence in Linka

- [ ] Build `linka::ProducerEvidence` for every submitted agent outcome.
- [ ] Use the stable namespace `orka`.
- [ ] Include at least:
  - [ ] attempt ID;
  - [ ] execution backend;
  - [ ] backend reference, when present;
  - [ ] observed start and finish timestamps;
  - [ ] observed exit code.
- [ ] Decide whether to include the configured command or a command digest.
      Avoid duplicating the full request if the attempt ID is sufficient to
      locate it in `.orka/`.
- [ ] Never put the transcript or mutable filesystem paths into Linka result
      metadata.
- [ ] Treat the executor report as authoritative; do not accept backend/model
      evidence asserted by the agent.
- [ ] Add tests proving Linka preserves but does not interpret the namespaced
      evidence.

Exit criterion: a Linka result identifies the Orka attempt that produced it,
while detailed execution records remain exclusively in `.orka/`.

## Phase 5 — Refactor the engine to use Linka directly

- [ ] Replace `Engine.graph: &dyn WorkGraph` with either:

  ```rust
  pub linka: LinkaWork<'a>
  ```

  or a direct `&linka::Store` if that produces clearer ownership. Prefer the
  concrete `LinkaWork` value when it keeps Linka calls out of lifecycle code.

- [ ] Change `Engine::run_node` to accept `&linka::NodeId`.
- [ ] Change `RunReport.node` and `RecoveryReport.node` to `linka::NodeId`.
- [ ] Change `run_next` to select through `LinkaWork::ready_for_machine`.
- [ ] Change attempt creation to persist `AttemptInput`.
- [ ] Plan and prepare the workspace from
      `input.snapshot.project.revision`.
- [ ] Change execution environment construction to publish the Linka node ID
      directly as `ORKA_NODE`.
- [ ] Change prompt generation to use the frozen prose in `AttemptInput`.
- [ ] Change `settle` to:
  - [ ] read and interpret the Orka outcome declaration;
  - [ ] validate the presence of a success workspace;
  - [ ] submit through concrete Linka integration;
  - [ ] include the captured execution report as producer evidence;
  - [ ] seal accepted success, accepted failure, or submission conflict;
  - [ ] leave operational failures unsealed and recoverable where safe.
- [ ] Pass the full `ExecutionReport` into settlement rather than only its exit
      code, so producer evidence is available during normal execution and
      recovery.
- [ ] Preserve current handling of a declared outcome plus a nonzero exit: the
      outcome is handled and backend failure remains separately reportable.
- [ ] Preserve conservative recovery:
  - [ ] never invent an outcome without exit evidence;
  - [ ] resubmit executed-but-unsealed attempts using the persisted snapshot;
  - [ ] keep submission idempotent or classify an already-recorded result
        deterministically;
  - [ ] never discard a dirty workspace;
  - [ ] clean only sealed attempts or attempts that cannot have a result.
- [ ] Review crash windows around output capture, Linka submission, Orka seal,
      and workspace cleanup. Add a durable submission marker if the Linka API
      cannot make recovery unambiguous from snapshot/result provenance.

Exit criterion: the engine contains no generic graph port and does not
translate between Orka and Linka graph models.

## Phase 6 — Keep the agent outcome contract Orka-owned

- [ ] Retain `DeclaredOutcome`, `DeclaredKind`, and the declaration/exit failure
      matrix in Orka.
- [ ] Rename the old graph-oriented `WorkOutcome` to an Orka-owned
      `AgentOutcome` or fold it into `Decision`.
- [ ] Do not deserialize agent-written TOML into `linka::ResultSubmission`.
- [ ] Keep these decisions in Orka:
  - [ ] missing declaration plus zero exit is a contract violation;
  - [ ] missing declaration plus nonzero exit is interrupted;
  - [ ] declared success or failure is eligible for Linka submission;
  - [ ] a nonzero exit accompanying a declaration is reported separately.
- [ ] Let the Linka integration module perform the only translation from
      `AgentOutcome` to Linka result operations.
- [ ] Validate that failure declarations cannot claim outputs.
- [ ] Define how an empty success output list differs from a graph-only answer,
      if Linka makes that distinction.

Exit criterion: agents declare an Orka execution outcome; only trusted Orka
code constructs Linka mutations.

## Phase 7 — Remove the duplicate graph layer

- [ ] Remove `WorkGraph` from `ports.rs`.
- [ ] Delete `orka/src/linka_graph.rs`.
- [ ] Delete `FakeWorkGraph`.
- [ ] Remove the `linka_graph` module export from `lib.rs`.
- [ ] Remove Orka's duplicate graph types:
  - [ ] `NodeId`;
  - [ ] `WorkItem` where replaced by a concrete ready-work view;
  - [ ] `DefinitionFingerprint`;
  - [ ] `ResultFingerprint`;
  - [ ] `ArtifactPin`;
  - [ ] `FrozenDependency`;
  - [ ] `FrozenInput`;
  - [ ] graph-oriented `WorkOutcome`;
  - [ ] `Submission`;
  - [ ] `SubmitOutcome`.
- [ ] Replace every remaining call site with Linka types or narrowly scoped
      Orka execution types.
- [ ] Remove conversion helpers such as `definition_fingerprint` and
      `expected_input`.
- [ ] Remove fake/real `WorkGraph` contract tests.
- [ ] Split or rename `ports.rs` after graph types are gone. Prefer focused
      modules such as `executor.rs` and `workspace.rs` if that is clearer.
- [ ] Search the Orka source for removed type names and generic graph-backend
      claims; require no matches except migration documentation.

Exit criterion: the only graph domain model visible in Orka is Linka's public
model.

## Phase 8 — Simplify CLI and workbench construction

- [ ] Replace `Workbench::graph()` with `Workbench::linka_store()`.
- [ ] Open `.linka/` using `linka::Store::open`.
- [ ] Build `GitWorkspaces` from `store.project_root()`.
- [ ] Parse CLI node arguments immediately as `linka::NodeId` and report
      Linka's validation error at the command boundary.
- [ ] Implement `orka ready` through concrete Linka selection.
- [ ] Update `orka attempts`, `orka show`, and `orka recover` for the new
      attempt schema and Linka node IDs.
- [ ] Keep workbench discovery based on the nearest `.linka/`; Orka is now
      explicitly a Linka-workbench application.
- [ ] Update CLI descriptions to say that Orka orchestrates isolated agent
      attempts for work in a Linka store.
- [ ] Ensure Orka configuration continues to decide agent command, mounts,
      network policy, and executor backend; none of these belong in Linka.

Exit criterion: CLI construction contains no adapter setup and makes Orka's
direct Linka dependency obvious.

## Phase 9 — Restructure tests

### Pure Orka tests

- [ ] Keep fast tests for:
  - [ ] attempt phase derivation;
  - [ ] atomic record writing and schema loading;
  - [ ] prompt construction from `AttemptInput`;
  - [ ] outcome declaration parsing;
  - [ ] declaration/exit decision matrix;
  - [ ] execution request construction;
  - [ ] workspace cleanup policy;
  - [ ] seal idempotency;
  - [ ] recovery classification that does not require graph mutation.
- [ ] Continue using `FakeExecutor` and `FakeWorkspaces` for these tests.
- [ ] Extract pure lifecycle helpers where necessary so graph fakes are not
      reintroduced merely to keep unit tests small.

### Concrete Linka integration tests

- [ ] Add a reusable fixture containing:
  - [ ] a temporary Linka workbench/store;
  - [ ] a temporary project Git repository;
  - [ ] valid Git identity configuration;
  - [ ] helpers to add, edit, link, complete, and fail nodes;
  - [ ] concrete `LinkaWork` construction.
- [ ] Test machine-only selection and ordering.
- [ ] Test snapshot and prompt context preparation.
- [ ] Test dependency and lineage handling.
- [ ] Test success with and without project outputs.
- [ ] Test failure evidence against the frozen snapshot.
- [ ] Test every structured Linka conflict.
- [ ] Test producer evidence.

### End-to-end Orka tests

- [ ] Keep a smaller suite using a real Linka store, real Git workspaces,
      `FsAttemptStore`, and `FakeExecutor`.
- [ ] Cover the full successful lifecycle.
- [ ] Cover graph-only success.
- [ ] Cover declared failure.
- [ ] Cover graph mutation during execution and stale-at-submit sealing.
- [ ] Cover output capture and publication expectations.
- [ ] Cover a nonzero backend exit with a declared outcome.
- [ ] Cover recovery from every durable attempt phase.
- [ ] Cover the crash window after Linka accepts a result but before Orka seals
      the attempt.
- [ ] Prove a second recovery pass performs no duplicate mutation.

Exit criterion: tests exercise actual Linka semantics where those semantics
matter, while execution and recovery policy remain cheaply unit-testable.

## Phase 10 — Documentation and architectural checks

- [ ] Update `orka/DESIGN.md`:
  - [ ] describe Orka as specifically orchestrating Linka work;
  - [ ] replace the `WorkGraph` boundary with direct use of Linka's public
        snapshot/submission API;
  - [ ] retain executor and workspace boundaries;
  - [ ] explicitly reject a generic graph-backend goal;
  - [ ] document `.linka/` and `.orka/` ownership;
  - [ ] document producer evidence and recovery responsibilities.
- [ ] Update `designs/SEPARATE_APPLICATIONS.md` to say that Orka depends on
      Linka's public library API and value types, rather than an Orka-owned
      generic application interface.
- [ ] Update `orka/README.md`, examples, and CLI help.
- [ ] Mark the old generic-port tasks in `orka/TASKS.md` as historical or link
      from them to this plan.
- [ ] Ensure Linka documentation describes `WorkSnapshot` and result submission
      as a stable public protocol for orchestrators and other callers.
- [ ] Add or retain dependency checks proving:
  - [ ] Linka builds and tests without Orka;
  - [ ] Linka does not import Orka types;
  - [ ] Orka never accesses Linka storage files except through Linka APIs;
  - [ ] Orka's executor and workspace policy does not move into Linka.

Exit criterion: documentation describes the architecture the code actually
implements, without backend-neutral language that the design no longer
supports.

## Final verification

- [ ] Format all changed Rust and Markdown where applicable.
- [ ] Run the Linka unit and integration test suite.
- [ ] Run the Orka unit and integration test suite.
- [ ] Run end-to-end Orka tests with a real Linka store and Git worktree.
- [ ] Run recovery tests for every recorded phase and relevant crash window.
- [ ] Build Linka independently of Orka.
- [ ] Build the full workspace.
- [ ] Verify searches for `WorkGraph`, `FakeWorkGraph`, `FrozenInput`,
      `DefinitionFingerprint`, and `linka_graph` find no live Orka code.
- [ ] Verify Orka contains no direct reads or writes beneath `.linka/`.
- [ ] Verify Linka contains no knowledge of `.orka/`, attempt phases, prompts,
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
