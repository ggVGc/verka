# Separate applications and stable interfaces

Status: accepted and implemented.

## Decision

The repository is a suite of five independently usable applications:

```text
 +----------+       +------+       +-------+
 | Orka Web +------>| Orka +------>| Driva |------>Bubblewrap
 +----+-----+       +--+---+       +-------+
      |                |  |
      v                |  v
   +-------+<----------+ +------+
   | Linka |             | Nota |-----> Git
   +-------+             +------+
```

**Linka** owns the node graph: definitions, dependency and lineage edges,
results, consumed-input pins, output references, and derived status,
readiness, blockers, provenance, and staleness.

**Driva** owns one isolated command execution: explicit mounts, network policy,
process I/O, exit status, and cleanup through a replaceable isolation backend.
It has no knowledge of agents, tasks, graphs, attempts, or reviews.

**Orka** owns multi-attempt orchestration: selecting Linka work, freezing its
inputs, creating durable attempts, constructing a Driva execution request,
handling its outcome, and version-safely reporting results to Linka. It does
not own isolation mechanics or Nota's review-entry representation. It also
owns the durable binding and workflow that coordinate Linka candidates and
verification nodes with Nota review branches.

**Orka Web** is Orka's local presentation layer. It combines Orka's ready
queue, attempt history and transcripts, candidate integration, and active
reviews with the Linka graph Orka is coordinating. It uses public application
APIs and never reads `.linka/` records directly.

**Nota** owns Git-native human review: it pins an exact Git subject and records
notes and suggested edits as commits on an append-only review branch. A narrow
provider trait resolves a reference to a repository and exact revision; Nota
does not interpret Linka identities or depend on another application here.

## Dependency rules

1. Linka, Driva, and Nota are standalone and depend on none of the other
   applications.
2. Orka depends on Linka's public library API and value types directly (it
   orchestrates Linka specifically, with no backend-neutral graph interface),
   on Driva through a narrow Orka-owned executor interface, and on Nota's
   Git-native review API.
3. Nota depends on none of the other applications. Integrations resolve their
   domain-specific identifiers before passing an exact Git revision to Nota.
4. Linka does not interpret Orka attempts or Nota review records.
5. Driva does not receive Linka node IDs or interpret Orka policy; it receives
   only the concrete command and capability grant chosen by its caller.
6. Nota verdicts are evidence, not candidate acceptance or publication policy;
   those remain explicit Linka operations exposed by Orka.
7. Orka Web depends on Orka's public records and services and Linka's public
   graph API. It owns no orchestration state of its own.

## Information flow

Orka reads a ready node and its permitted context from Linka and freezes that
input as a `linka::WorkSnapshot` in an attempt, then asks Driva to run an agent
with precisely selected mounts and network policy. Driva returns execution
evidence and an exit outcome. Orka submits success or failure against that exact
snapshot through Linka's version-checked `capture_submission`, which revalidates
every frozen input before recording anything.

For a coordinated review, Orka creates a Linka verification node and freezes
its input, records the binding under `.orka/reviews/`, and asks Nota to start a
branch at the candidate's exact Git artifact. Reviewers add notes and staged
suggestions through Nota. Orka later reads that Git evidence and submits a
version-checked result for the verification node. A review verdict does not by
itself accept, reject, or publish the candidate.

## Persistence ownership

Each application writes only through storage it owns or an explicit adapter:

- Linka owns its graph store (default `.linka/`).
- Driva owns ephemeral isolation runtime data until command cleanup completes.
- Orka owns durable orchestration attempts, review bindings, and audit evidence
  under `.orka/`; evidence needed to inspect a produced output is also copied
  durably through Linka's generic, opaque node-attachment interface.
- Nota stores its review marker and entries as commits on a Git branch; note
  bodies also appear as files under `.nota/notes/` on that branch.

Sharing a Git repository does not merge these schemas. Cross-application
references are stable opaque identifiers or version pins.

## Repository layout

```text
linka/       graph library and CLI
driva/       standalone isolated command runner
orka/        Linka + Driva orchestration
orka-web/    local web interface for an Orka workbench
nota/        standalone review application
```

## Architectural verification

- Linka builds and tests without Driva, Orka, or Nota.
- Driva policy tests use a fake isolation backend and no Linka types.
- Orka tests use a real Linka store (Orka orchestrates Linka directly) and can
  substitute fake isolated-executor and workspace implementations.
- Orka's review tests exercise the real Linka and Nota integration while
  verifying that Nota sees only Git subjects.
- Nota tests exercise review branches, note commits, suggestion commits, and
  loading reviews without any Linka dependency.
