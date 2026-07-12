# llaundry-work (parked)

The execution harness / orchestrator for llaundry task graphs. This project
collects everything execution- and review-related that used to be woven
through the composed llaundry workspace:

The intended architecture, including its attempt-scoped MCP server and
capability model, is recorded in [DESIGN.md](DESIGN.md).

* `src/lib.rs`, `src/config.rs`, `src/backend*` — the former
  `crates/llaundry-work` crate: durable execution attempts, worktree
  workspaces, backends (`claude`, `codex`), driver config.
* `src/review.rs` — the former `crates/llaundry-review` crate: candidates,
  decisions, review-gated publication.
* `src/harness.rs` — the execution/review halves of llaundry's old `ops`
  module (prepare/finalize attempts, create/decide reviews, publication
  recovery, dispatchability policy).
* `src/harness_tests_reference.rs` — the tests that covered those operations.
* `src/protocol.rs` — the versioned request/response envelope for
  out-of-process graph adapters (from the former `llaundry-core`).
* `src/bin/llaundry-work.rs` — the work driver CLI.

## Status

**This project is parked and does not build.** The sources still refer to
items from the old workspace layout (`llaundry_core::…`, the composed
`Store`/`Vcs`, `Store::lock_execution`, `FsGraphStore`).

## Plan

Define a **task store trait** here — the seam the harness needs from any task
graph (read definitions, submit results, pin versions, resolve readiness) —
and implement it with an adapter for the `llaundry` library (path dependency
`../llaundry`). The harness, review, and backend code then depend only on the
trait, and llaundry stays free of execution/review vocabulary.
