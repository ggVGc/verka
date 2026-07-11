# Separate applications and stable interfaces

Status: implemented. The root `llaundry` package composes the three
application crates into the integrated Git workbench and its frontends; new
integrations depend on the application crates directly.

## 1. Decision

Llaundry is a work-provenance graph, not an execution environment or a code
review system. The repository contains three applications with a one-way
dependency graph:

```text
                 +----------------+
                 | llaundry-core  |
                 +----------------+
                    ^          ^
                    |          |
          +----------------+  +-----------------+
          | llaundry-work  |  | llaundry-review |
          +----------------+  +-----------------+
                    ^          ^
                    +----+-----+
                         |
                  frontend binaries
```

`llaundry-core` owns nodes, dependency and lineage edges, immutable definition
and result versions, consumed-input pins, output artifact references, and the
derived status, readiness, blocker, provenance, and staleness queries.

`llaundry-work` owns execution attempts, agent backends, transcripts,
workspace isolation and retention, and collection of result evidence. It may
consume and submit work through the core API, but core may not depend on it.

`llaundry-review` owns candidates, decisions, reviewer suggestions,
publication policy, target-ref movement, and recoverable publication
transactions. It may refer to core nodes and result versions, but core may not
interpret review state.

Separate crates enforce these dependency directions. They may remain in one
repository and one release until independent versioning or distribution is
useful.

## 2. Core semantics

A core result answers four questions only:

1. Which exact node definition did this work fulfill?
2. What was the generic outcome?
3. Which node versions, outputs, and context identities did it consume?
4. Which artifact, if any, did it produce?

`done` means that a successful result covers the current definition. Approval
or publication does not alter that fact. Core readiness is therefore:

```text
node is not done
and every depends_on node is done and not stale
```

Execution policy may decide not to dispatch a core-ready node. Review policy
may decide that an output is not publishable. Those are separate predicates.

Reviews can still be represented as ordinary nodes when an adapter wants their
work and narrative in the graph. Structured decisions are owned by the review
application and linked to such nodes; there is no review node subtype in core.

## 3. Interfaces

The in-process Rust interfaces are the reference contracts. External tools use
versioned JSON envelopes over standard input/output; MCP remains an
agent-facing adapter, not the foundational application protocol.

### 3.1 Work graph

The work provider exposes definition lookup, readiness, dependency snapshots,
and result submission. Submission accepts producer evidence as opaque
namespaced data. Trusting a producer is authorization at the adapter boundary,
not a special meaning of `author = machine` in the graph model.

### 3.2 Artifacts

Core uses opaque artifact references and an artifact resolver. The initial
resolver uses Git commits. It provides identity/existence, path listing, and
drift checks; branch creation, worktrees, and publication are deliberately not
part of this interface.

### 3.3 Review

A candidate pins a subject, submitted result version, and artifact. A review
decision pins exactly that candidate. Publication uses compare-and-swap
semantics against an expected previous target and is recoverable across the
review store and artifact repository.

### 3.4 Execution

An attempt pins a work item, definition version, input artifact, executor, and
workspace identity. Workspace paths and backend exit evidence are local
operational records; a submitted result contains only portable provenance.

## 4. Persistence ownership

Each application owns a top-level namespace, and nothing writes outside its
own:

```text
.llaundry/nodes/          core graph — definitions and results
.llaundry/execution/      work runner — attempts, transcripts, final records
.llaundry/reviews/        review — candidates and decisions
.llaundry/publications/   review — publisher transaction log
```

A core result is the single schema for `nodes/<id>/result.toml`: definition
version, outcome, consumed pins, context pins, output artifact, and opaque
namespaced producer evidence. Execution and review state that used to ride
along inside the node result now live under the owning namespace — the
producing attempt and backend are `llaundry-work` producer evidence; the
reviewed candidate, decision, and publication intent are `llaundry-review`
records. Core never interprets any of it.

## 5. Frontends over the applications

The existing command names are frontends composing the application crates.
Each frontend reads and writes only through the owning crate's store: the CLI,
MCP, TUI, and visualization render core results plus `review_info`/`worked_by`
accessors; the work driver drives `llaundry-work` attempts and
`llaundry-review` candidates. No frontend reaches across an application
boundary or reconstructs another application's state from the core node.

## 6. Architectural tests

The workspace must ensure:

* `llaundry-core` has no dependency on either application crate;
* core model source contains no attempt, worktree, candidate branch, review
  decision, or publication fields;
* core status and readiness tests do not construct review state;
* runner tests can use a fake work provider;
* review tests can use a fake candidate source and publisher;
* Git is one artifact/workspace/publisher implementation, not the interface
  definition itself.
