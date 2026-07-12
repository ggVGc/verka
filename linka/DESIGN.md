# Linka design

## Purpose

Linka is a standalone, git-versioned graph of work nodes. It records what work
means, how work items relate, and what results were produced. It does not run
agents, manage containers, orchestrate attempts, or interpret reviews.

## Model

A node has an immutable identity and versioned definition. Directed edges
express dependencies, lineage, and explicit context. A result pins the exact
definition and inputs it covered and may declare output artifacts.

Graph state is derived, never stored as one `status` value. It has three
independent dimensions:

- **Recorded outcome** is `open`, `succeeded`, or `failed`. `open` means that
  no result is recorded; the other values report the outcome of the recorded
  result. An outcome is historical evidence, not a statement that the node is
  currently complete.
- **Currency** is `current` or `stale`. Recorded evidence is current only while
  it still describes the current node definition, consumed node versions and
  outputs, explicit context, and the node's own output. A node with no result
  is current because it has no recorded evidence that can have gone stale.
- **Workability** is `complete`, `ready`, or `blocked`. This dimension answers
  whether the current work is already satisfied or can be performed now.

These dimensions are related by the following authoritative rules:

1. A node is **complete** if and only if it has a successful result that covers
   its current definition, all consumed inputs and context, and a still-valid
   output. In other words, only `succeeded` plus `current` can be complete.
2. A node is **ready** if it is not complete and every node named by a current
   `depends_on` edge is complete.
3. Every other valid node is **blocked**. Its blockers are the current
   `depends_on` targets that are not complete.
4. A `derived_from` edge records lineage. Its pinned versions participate in
   provenance and can make existing evidence stale, but the current state of a
   `derived_from` target never blocks work.

Successful results may only be accepted for nodes that are ready immediately
before the result is recorded. Recording the result changes a ready node to a
complete node. Failed evidence may be recorded regardless of readiness: it
describes what happened, but never makes the node complete or removes its
blockers.

When a definition, consumed node, context value, or output changes, any
recorded result becomes stale whether it succeeded or failed. A stale failure
remains failed historical evidence; it does not revert to `open`. Like an open
node or a current failure, it is ready when all required dependencies are
complete and blocked otherwise. Re-recording a result replaces the evidence
whose currency is evaluated; retry and evidence-retention policy belong to
applications.

The truth table below summarizes valid graph states. "Dependencies complete"
refers only to current `depends_on` edges. A successful/current result already
implies that its consumed required dependencies and output are valid.

| Recorded outcome | Currency | Dependencies complete | Workability | Meaning |
| --- | --- | --- | --- | --- |
| open | current | yes | ready | No result has been recorded and work can start. |
| open | current | no | blocked | No result has been recorded, but required work is incomplete. |
| failed | current or stale | yes | ready | Failure is evidence; the current work can be tried. |
| failed | current or stale | no | blocked | Failure is evidence and required work is incomplete. |
| succeeded | current | necessarily yes | complete | The result covers the current node and all of its inputs and output. |
| succeeded | stale | yes | ready | Prior success is evidence, but the current work must be redone. |
| succeeded | stale | no | blocked | Prior success is evidence, and required work must be completed first. |
| any or unreadable | unknown | unknown | error | Corrupt or unreadable facts have no graph state. |

Missing facts with defined semantics are not corruption: an absent result is
`open`, a context path proven absent is stale, and an artifact proven absent is
stale. In contrast, failures to read or parse definitions or results, failures
to inspect context, and artifact-backend failures are errors. Queries must
return those errors rather than converting them to `open`, `ready`, `blocked`,
or `stale`.

Approval, dispatch eligibility, and publication are policies belonging to
other applications and cannot change these graph facts.

## Storage

The default store is `.linka/` in a Git workbench. Node definitions, results,
and logs use inspectable TOML and Markdown. Mutations are committed so Git
provides history, integrity, blame, and distribution; Linka provides the graph
semantics Git cannot express.

Definitions and results are never overwritten as hidden mutable state. Stored
facts are minimal; readiness, blockers, dependents, provenance, and staleness
are computed from them.

## Interfaces

The Rust library is the reference interface. The `linka` CLI exposes the same
operations to people and scripts. An agent-facing protocol may adapt those
operations, but protocol-specific concepts do not enter the graph model.

Orka consumes a narrow graph interface for reading ready work, freezing
versioned input, and submitting version-checked results. Nota may use an
optional adapter to persist or link review records, but Linka never interprets
their schema.

## Non-goals

- Starting or supervising agent processes.
- Docker, worktree, or network isolation policy.
- Scheduling and retry policy.
- Review comments, suggested edits, approval, or publication.
- Requiring Orka, Driva, or Nota for normal CLI/library use.
