# llaundry-work reference

Parked source from the old combined execution/review application (formerly
called `llaundry-work`). This code does not build and is kept only as
reference material for the fresh Orka implementation — see
[`../orka/DESIGN.md`](../orka/DESIGN.md) for the accepted design.

Rough map of where responsibilities land in the new direction:

- Docker and raw agent-session lifecycle code (`harness.rs`, backends) is
  superseded by Driva's runner interface.
- Review, candidate, suggestion, and publication code (`review.rs`,
  `nota-reference.rs`) belongs to Nota.
- Graph access patterns (`linka-graph-reference.rs`) belong to Linka
  adapters.
- Attempts, scheduling, recovery, and the Linka/Driva composition are Orka's
  responsibility and will be reimplemented fresh in `orka/`.

Nothing here is a build target; do not extend this code.
