# Separate applications and stable interfaces

Status: implemented. The root `llaundry` package is the compatibility facade
and integrated frontend; new integrations depend on the three application
crates directly.

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

Each application owns a top-level namespace:

```text
.llaundry/nodes/          core
.llaundry/execution/      work runner
.llaundry/reviews/        review decisions and candidates
.llaundry/publications/   review publisher transaction log
```

Legacy `attempts/` and review fields remain readable during migration. New
writes use the owning namespace. Migration must not rewrite historical Git
commits or make recorded output commits unreachable.

## 5. Compatibility and migration

The existing command names remain frontends over the new crates. Compatibility
is provided at command and persisted-data boundaries, not by allowing core to
depend on runner or review types.

Implemented migration order:

1. Establish the Cargo workspace and one-way crate graph.
2. Extract generic graph model, persistence, queries, and artifact interfaces.
3. Extract execution records and workspace lifecycle.
4. Extract review decisions and publication recovery.
5. Rewire CLI, MCP, TUI, visualization, and work driver.
6. Read legacy records and write the separated schema.
7. Retain compatibility readers until a later schema-major release.

The integrated frontend dual-writes owner-specific execution and review
records while retaining legacy node/attempt records. `llaundry-core` reads
legacy node/results directly, and `llaundry-work`/`llaundry-review` fall back
to the legacy namespaces. No historical commit is rewritten.

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
