# Linka, Driva, Orka, and Nota

This repository contains four small applications for graph-driven agent work.
They are separate applications with narrow, one-way dependencies rather than a
single framework.

## Applications

- `linka/` — a git-versioned node graph. Definitions and results are plain
  TOML/Markdown files; status, readiness, and staleness are derived rather than
  stored. Linka is usable as a library or CLI and has no dependency on the
  other applications.
- `driva/` — a standalone isolated command runner. It exposes only explicit
  host mounts, disables networking by default, and delegates execution to a
  replaceable isolation backend. Podman is the default; Docker is also
  supported.
- `orka/` — the orchestrator. It uses Linka to find and track work and Driva to
  execute agent commands in isolation. It owns orchestration policy and durable attempts,
  but no review workflow.
- `nota/` — a standalone review application for comments and suggested edits.
  Review persistence is behind a storage trait. Initial backends store records
  either as ordinary repository files or through a Linka adapter.

`linka-viz/` is a small Linka-specific graph viewer, not another domain
application.

## Dependency direction

```text
Orka ----> Linka
  |
  +------> Driva ----> Podman / Docker

Nota ----> ReviewStore <---- repository files
                       <---- Linka adapter ----> Linka
```

Linka and Driva know nothing about Orka or Nota. Orka composes Linka and Driva.
Nota's domain code knows only its own storage interface; Linka support is an
adapter at the edge.

## Design documents

- [`linka/DESIGN.md`](linka/DESIGN.md)
- [`driva/DESIGN.md`](driva/DESIGN.md)
- [`orka/DESIGN.md`](orka/DESIGN.md)
- [`nota/DESIGN.md`](nota/DESIGN.md)
- [`designs/SEPARATE_APPLICATIONS.md`](designs/SEPARATE_APPLICATIONS.md)

The source split is in progress. Linka is the existing working core. Orka is
implemented fresh against its design; Driva and Nota begin as explicit designs
so their code can be extracted behind the contracts described above.
`llaundry-work-reference/` holds parked, non-building source from the former
combined execution/review project (`llaundry-work`), kept as reference only.
