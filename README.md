# Linka, Driva, Orka, and Nota

This repository contains four small applications for graph-based work,
isolated command execution, orchestration, and Git-native review. They are
separate applications with narrow, one-way dependencies rather than a single
framework.

## Applications

- `linka/` — a git-versioned node graph. Definitions and results are plain
  TOML/Markdown files; status, readiness, and staleness are derived rather than
  stored. Linka is usable as a library or CLI and has no dependency on the
  other applications.
- `driva/` — a standalone isolated command runner. It exposes only explicit
  host mounts, disables networking by default, and delegates execution to a
  replaceable isolation backend. Bubblewrap is the default; Podman and Docker
  are also supported.
- `orka/` — the orchestrator. It uses Linka to find and track work and Driva to
  execute agent commands in isolation. It owns orchestration policy, durable
  attempts, candidate-oriented commands, and the coordination between Linka
  verification nodes and Nota reviews.
- `nota/` — a standalone Git-native review application. A review is an
  append-only branch: Markdown note commits and ordinary project commits form
  its record. Nota knows Git revisions, but not Linka candidates or nodes.

`linka-viz/` is a small Linka-specific graph viewer, not another domain
application.

## Dependency direction

```text
linka-viz ----> Linka <---- Orka ----> Driva ----> Bubblewrap / Podman / Docker
                            |
                            +-------> Nota ----> Git
```

Linka, Driva, and Nota have no dependencies on one another. Orka is the only
application that composes them: it resolves a Linka candidate to an exact Git
commit, starts or reads the Nota branch, and submits the resulting evidence to
a Linka verification node. Nota never interprets Linka identities, while Linka
never interprets Nota's review data.
