# Separate applications and stable interfaces

Status: accepted; source extraction in progress.

## Decision

The repository is a suite of four independently usable applications:

```text
                 +-------+
                 | Linka |
                 +---^---+
                     |
                 +---+---+       +-------+
                 | Orka  +------>| Driva |----> Docker
                 +-------+       +-------+

                 +-------+
                 | Nota  |----> ReviewStore
                 +-------+          |     |
                               files     Linka adapter
```

**Linka** owns the node graph: definitions, dependency and lineage edges,
results, consumed-input pins, output references, and derived status,
readiness, blockers, provenance, and staleness.

**Driva** owns one isolated command execution: explicit mounts, network policy,
process I/O, exit status, and cleanup through a replaceable isolation backend.
It has no knowledge of agents, tasks, graphs, attempts, or reviews.

**Orka** owns multi-session orchestration: selecting Linka work, freezing its
inputs, creating durable attempts, constructing a Driva execution request,
handling its outcome, and version-safely reporting results to Linka. It does
not own Docker mechanics or review state.

**Nota** owns human review: review subjects, comments, suggested edits, and
resolution/follow-up state. Its domain logic targets a `ReviewStore` trait.
File and Linka-backed persistence are adapters, not assumptions in the model.

## Dependency rules

1. Linka and Driva are standalone and depend on none of the other applications.
2. Orka depends on Linka's public library API and value types directly (it
   orchestrates Linka specifically, with no backend-neutral graph interface),
   and on Driva through a narrow Orka-owned executor interface.
3. Nota's core depends only on `ReviewStore`; its optional Linka adapter may
   depend on Linka.
4. Linka does not interpret Orka attempts or Nota review records.
5. Driva does not receive Linka node IDs or interpret Orka policy; it receives
   only the concrete command and capability grant chosen by its caller.
6. Orka contains no review decisions, candidate publication, or review UI.

## Information flow

Orka reads a ready node and its permitted context from Linka and freezes that
input as a `linka::WorkSnapshot` in an attempt, then asks Driva to run an agent
with precisely selected mounts and network policy. Driva returns session
evidence and an exit outcome. Orka submits success or failure against that exact
snapshot through Linka's version-checked `capture_submission`, which revalidates
every frozen input before recording anything.

Nota loads a review through `ReviewStore`, records comments or suggested
edits, and may mark an item as requiring follow-up. With the Linka adapter,
that state can be represented or linked in the graph. With the file backend,
it remains ordinary versionable files in the reviewed repository.

## Persistence ownership

Each application writes only through storage it owns or an explicit adapter:

- Linka owns its graph store (default `.linka/`).
- Driva owns ephemeral isolation runtime data until command cleanup completes.
- Orka owns durable orchestration attempts and audit evidence.
- Nota owns review records through `ReviewStore`.

Sharing a Git repository does not merge these schemas. Cross-application
references are stable opaque identifiers or version pins.

## Repository layout

```text
linka/       graph library and CLI
linka-viz/   Linka graph viewer
driva/       standalone container session runner
orka/        Linka + Driva orchestration
nota/        standalone review application
```

## Architectural verification

- Linka builds and tests without Driva, Orka, or Nota.
- Driva policy tests use a fake isolation backend and no Linka types.
- Orka tests use a real Linka store (Orka orchestrates Linka directly) and can
  substitute fake isolated-executor and workspace implementations.
- Nota domain tests run against an in-memory `ReviewStore` contract suite;
  file and Linka adapters run the same persistence contract tests.
- Dependency checks reject imports that reverse the arrows above.
