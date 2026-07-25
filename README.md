# Linka, Driva, Genta, Orka, Nota, and their interfaces

This repository contains several small applications for graph-based work,
isolated command execution, coding-agent knowledge, orchestration, and
Git-native review. They are separate applications with narrow, one-way
dependencies rather than a single framework.

## Applications

- `linka/` — a git-versioned node graph. Definitions and results are plain
  TOML/Markdown files; status, readiness, and staleness are derived rather than
  stored. Linka is usable as a library or CLI and has no dependency on the
  other applications.
- `linka-tui/` — a terminal interface over the Linka library. It presents
  nodes, candidates, verifications, derived queues, associations, and the full
  set of graph and candidate actions without adding UI concerns to Linka.
- `driva/` — a standalone isolated command runner. It exposes only explicit
  host mounts, disables networking by default, and delegates execution to a
  replaceable isolation backend. Bubblewrap is the backend.
- `genta/` — a standalone library of coding-agent knowledge: launch profiles
  carrying command lines, mounts, and environment; the wire protocols agents
  speak and their decoding into a stable event vocabulary; the `codex
  app-server` handshake; and transcript rendering. Genta is transport-agnostic
  and never spawns a process — hosts launch agents through their own executor
  and feed lines to Genta's decoders.
- `orka/` — the orchestrator. It uses Linka to find and track work, Genta to
  describe the agent and decode its output, and Driva to execute agent commands
  in isolation. It owns orchestration policy, durable attempts,
  candidate-oriented commands, and the coordination between Linka verification
  nodes and Nota reviews.
- `orka-tui/` — a terminal interface over Orka and the Linka graph it
  orchestrates.
- `orka-web/` — Orka's local web interface. It combines Orka's ready queue,
  attempts and transcripts, candidates, and active reviews with the Linka graph
  being orchestrated.
- `nota/` — a standalone Git-native review application. A review is an
  append-only branch: Markdown note commits and ordinary project commits form
  its record. Nota knows Git revisions, but not Linka candidates or nodes. The
  implemented scope is the prototype described in
  [`nota/PROTOTYPE_V1.md`](nota/PROTOTYPE_V1.md).

## Dependency direction

Each application depends on exactly what is listed beside it, and on nothing
else here:

```text
Linka TUI  ---->  Linka
Orka TUI   ---->  Orka, Linka
Orka Web   ---->  Orka, Linka
Orka       ---->  Linka, Driva, Genta, Nota
Driva      ---->  Bubblewrap
Nota       ---->  Git
```

Linka, Driva, Genta, and Nota are leaves: they depend on no other application
here, and none of them depends on another. Orka is the only application that
composes them: it resolves a Linka candidate to an exact Git commit, starts or
reads the Nota branch, and submits the resulting evidence to a Linka
verification node. Nota never interprets Linka identities, while Linka never
interprets Nota's review data. Genta describes agents but runs nothing, so Orka
pairs a Genta profile with a Driva execution request. Orka TUI and Orka Web are
presentation layers over that composition; they read Orka's public records and
services and Linka's public graph API rather than either application's on-disk
representation directly.

The `styra/` and `styra-server/` pair is a separate application with its own
[README](styra/README.md); it shares the Driva and Genta leaves but has no
relationship to Linka, Orka, or Nota. `linka-viz/` depends on nothing here.
