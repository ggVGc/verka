# Linka design

## Purpose

Linka is a standalone, git-versioned graph of work nodes. It records what work
means, how work items relate, and what results were produced. It does not run
agents, manage containers, orchestrate attempts, or interpret reviews.

## Model

A node has an immutable identity and versioned definition. Directed edges
express dependencies, lineage, and explicit context. A result pins the exact
definition and inputs it covered and may declare output artifacts.

Status is derived from definitions and results:

```text
done  = a successful result covers the current definition and inputs
ready = not done and every dependency is done and current
stale = a covered definition or consumed input has changed
```

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
