# Orka implementation tasks

Implementation plan for Orka per [`DESIGN.md`](DESIGN.md),
[`../designs/SEPARATE_APPLICATIONS.md`](../designs/SEPARATE_APPLICATIONS.md),
[`../designs/DURABLE_EXECUTION_ATTEMPTS.md`](../designs/DURABLE_EXECUTION_ATTEMPTS.md),
and [`../designs/ISOLATED_WORKTREE_EXECUTION.md`](../designs/ISOLATED_WORKTREE_EXECUTION.md).

## Starting point

- linka provides `ready_nodes`/`first_ready_for`, `node_state`, `node_version`,
  and pinned completion (`ops::complete` freezes definition version, consumed
  dependency versions, context pins, and input commit into `ResultMeta`).
- driva provides Stage 1 synchronous execution (`Isolation`, `execute()`,
  podman/docker backends); Stage 2 durable sessions are in progress.
- orka starts fresh; the former combined implementation is reference material
  in `../llaundry-work-reference/`.

## Phase 0 — Crate skeleton and boundary ports

- [x] `orka/Cargo.toml` with path dependencies on the `linka` and `driva`
      libraries.
- [x] `src/ports.rs` defining Orka's two ports. All types crossing them are
      Orka-owned; adapters translate. No `linka::model` or
      `driva::ExecutionRequest` in port signatures.

  ```rust
  trait WorkGraph {
      fn select_ready(&self, ...) -> Result<Vec<WorkItem>>;
      fn freeze(&self, id: &NodeId) -> Result<FrozenInput>;
      fn submit(&self, sub: VersionCheckedSubmission) -> Result<SubmitOutcome>;
  }

  trait IsolatedExecutor {
      fn run(&self, request: ExecutionSpec) -> Result<ExecutionReport>;
  }
  ```

- [x] `FakeWorkGraph` and `FakeExecutor` test doubles so every later phase is
      testable without podman or a real graph.
- [x] Dependency rule holds: orchestration logic never reads `.linka/` or
      invokes a container engine directly.

## Phase 1 — Durable attempt store

An attempt is written before external side effects; build durability first.

- [x] `AttemptStore` trait plus `FsAttemptStore` under
      `.orka/attempts/<attempt-id>/` (Orka-owned storage; no migration from
      `.linka/execution/` since orka starts fresh).
- [x] Per-attempt layout:
  - `attempt.toml` — frozen Linka input, chosen mounts/network/command,
    workspace path, candidate branch, preparation markers.
  - `request.toml` — the exact Driva request, recorded before start.
  - `transcript.log` — authoritative transcript captured by Orka.
  - `evidence.toml` — Driva `ExecutionEvidence` plus exit status.
  - `submission.toml` — sealed final state: submitted, failed, interrupted,
    or stale-at-submit.
- [x] State advances by writing new files, never rewriting old ones; sealing
      is idempotent.
- [x] Tests simulate a crash after each lifecycle step and assert the attempt
      is classifiable and recoverable (compensated-transaction property).

## Phase 2 — Linka adapter for `WorkGraph`

- [x] `select_ready` / `freeze` over existing ops (`ready_nodes`,
      `node_version`, the same pinning `complete` performs).
- [x] Version-checked submit. Gap: `linka::ops::complete` takes no expected
      versions. Preferred fix: add a linka op (`complete_checked` or an
      `expected` parameter) returning a typed `Stale` error when current
      definition/dependency versions differ from the frozen ones, so the
      check happens where the store lock lives (no TOCTOU window).
      Fallback: adapter-side check-then-complete, acceptable only while Orka
      is the sole writer.
- [x] Failed/interrupted outcomes map to `ops::fail`.
- [x] Contract tests run identically against `FakeWorkGraph` and the real
      adapter over a throwaway git repo + `.linka/` store, including the
      stale-at-submit path (mutate the node between freeze and submit).

## Phase 3 — Driva adapter for `IsolatedExecutor`

- [x] Wrap Stage 1 `driva::execute()` synchronously; translate
      `ExecutionSpec` into `ExecutionRequest`.
- [x] Capture stdout/stderr into the attempt's `transcript.log` via
      `ExecutionIo` — Orka owns the authoritative transcript.
- [x] Persist `ExecutionEvidence` (backend, reference, effective policy,
      timestamps) into `evidence.toml`; backend/model evidence comes from the
      harness, not agent claims.
- [ ] Later, isolated change: adopt Driva Stage 2 `SessionRunner` for
      detachable/recoverable executions without changing the port.

## Phase 4 — Workspace policy (worktree isolation)

Orka, not Driva, owns workspace geometry.

- [x] Create a linked worktree from the frozen input commit on a candidate
      branch named by attempt id; record it in `attempt.toml` before
      creation, mark preparation complete after.
- [x] The Driva request mounts only the worktree read-write plus explicitly
      chosen read-only context; `.linka/` and the user's `project/` checkout
      are never mounted.
- [x] On success, capture declared outputs as one commit parented on the
      input commit; retain the ref so cleanup cannot orphan it.
- [x] Cleanup removes worktrees only for sealed attempts and never deletes
      dirty trees.

## Phase 5 — Outcome contract, orchestration loop, and CLI

- [x] Agent outcome contract (Orka-internal): the agent writes a well-known
      `outcome.toml` (outcome, notes, declared output paths) inside a
      designated writable mount. Orka combines it with exit evidence:
  - outcome present + exit 0 → submit;
  - outcome present + nonzero exit → submit, then report backend failure;
  - no outcome + exit 0 → contract violation (finalization error);
  - no outcome + nonzero exit → seal interrupted attempt;
  - failed/graph-only outcome → seal without project-output submission.
- [x] `orka run [NODE]` — full lifecycle: select → freeze → record attempt →
      prepare worktree → record Driva request → execute → capture evidence →
      re-check frozen versions → submit or seal as stale/failed.
- [x] `orka attempts` / `orka show ATTEMPT` — list and inspect attempts.
- [x] `orka recover` — classify each attempt (prepared-but-not-run,
      ran-but-unsealed, sealed-but-unsubmitted, submitted) and complete the
      remaining idempotent steps; recreate safe missing workspaces; never
      invent results or discard dirty files.
- [x] `orka.toml` — agent command template, image/backend selection, default
      read-only context mounts, network policy. Orka config decides policy;
      driva config stays mechanism.

## Phase 6 — Scoped agent authority (deferrable)

- [x] Initially: the file-mount grant plus the outcome-file contract gives
      the agent zero graph access.
- [ ] Later: an attempt-scoped graph proxy (MCP authorized only by attempt
      id) if agents need to read node context or ask questions mid-run —
      through Orka, never as a `.linka/` mount.

## Milestones

1. **M1 (Phases 0–1):** crate builds; attempt lifecycle unit-tested against
   fakes with crash-recovery coverage.
2. **M2 (Phase 2 + linka `complete_checked`):** end-to-end freeze→submit
   against a real temp workbench, including stale rejection.
3. **M3 (Phases 3–4):** `orka run` executes a trivial command in a
   podman-isolated worktree and lands a version-checked result in linka.
4. **M4 (Phase 5):** real agent command, outcome contract, `recover`, and
   the failure-matrix behaviors.
5. **M5 (Phase 6, optional):** scoped graph tools; Driva Stage 2 sessions
   for detachable runs.

## Open decisions

1. **Attempt storage location** — `.orka/` (recommended) vs. the migratory
   `.linka/execution/`.
2. **Version-checked submit in linka** — small linka API addition
   (recommended) vs. adapter-side check-then-complete.
3. **Sync vs. durable execution first** — start with Stage 1 synchronous
   `execute()`; driva Stage 2 work should not block M1.
