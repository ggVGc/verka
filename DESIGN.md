# Application suite design

The accepted architecture is five applications with explicit boundaries:

- **Linka** stores the versioned node graph.
- **Driva** runs one command through a replaceable isolation backend.
- **Orka** uses Linka and Driva to orchestrate work.
- **Orka Web** presents an Orka workbench in a local web interface.
- **Nota** records review comments, suggested edits, and follow-up requests
  through a pluggable storage backend.

The authoritative application designs live beside their applications:

- [`linka/DESIGN.md`](linka/DESIGN.md)
- [`driva/DESIGN.md`](driva/DESIGN.md)
- [`driva/STAGE2_IMPLEMENTATION.md`](driva/STAGE2_IMPLEMENTATION.md) — durable
  session production-hardening plan
- [`orka/DESIGN.md`](orka/DESIGN.md)
- [`orka-web/README.md`](orka-web/README.md)
- [`nota/DESIGN.md`](nota/DESIGN.md)

Cross-application dependency, information-flow, persistence, and migration
rules are in [`designs/SEPARATE_APPLICATIONS.md`](designs/SEPARATE_APPLICATIONS.md).

Older topic designs under `designs/` remain useful implementation background,
but application ownership is determined by the four documents above.
