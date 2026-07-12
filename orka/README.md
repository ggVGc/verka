# Orka (parked)

Orka is the orchestrator: it uses Linka to track work and Driva to run agent
sessions. See [`DESIGN.md`](DESIGN.md) for the accepted boundary.

This directory contains parked source inherited from the former combined
execution/review application and does not build yet. During extraction:

- Docker and raw agent-session lifecycle code moves behind Driva's runner
  interface.
- Review, candidate, suggestion, and publication code moves to Nota.
- Orka retains attempts, scheduling, scoped graph access, recovery, and the
  adapters that compose Linka with Driva.

The `nota-reference.rs`, `linka-graph-reference.rs`, `review.rs`, and related
reference tests are migration material, not exposed Orka binaries or accepted
Orka responsibilities.
