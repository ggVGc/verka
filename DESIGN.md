# llaundry — design

This document describes llaundry's data model, the reasoning behind it, and the
tools that operate on it. It is a record of the design discussion as much as a
specification.

llaundry's premise (see `README.md` / `ideas.wiki`): **the prompt history is the
story, not the output code.** Work is driven by a graph of nodes — tasks,
implementations, builds, verifications — each carrying the description that
drives the work and, once worked, a record of what happened: the context
consumed, the output produced, and the narrative. The two questions the system
must answer are:

1. *What happened during the work on this node?*
2. *Which follow-up work depends on some previous work — and is it still valid?*

---

## 1. Goals

* **Text-based and diff-friendly.** Every record is human-readable text, and any
  change produces a minimal, reviewable diff.
* **Git is the only versioning layer.** Git owns content integrity, history,
  blame, and distribution. llaundry stores nothing git could store for it — no
  object store, no refs, no hashes of its own design.
* **One unit of work per node.** A node is worked once; the work produces a
  single result record and (at most) a single output commit encompassing
  everything it produced. Rework replaces the record; git history keeps every
  earlier attempt.
* **Verifiable dependencies.** A result records exactly which versions of other
  nodes (and which of their outputs) the work was built against, so an upstream
  change flags the downstream work with an explicit, diffable reason.
* **Everything derivable is derived.** Status, readiness, blockedness,
  staleness, reverse dependencies, and commit→node provenance are all computed
  from the files on each query — never stored, so they can never drift.
* **Minimal.** The core is a small library; a CLI, a TUI, an MCP server, and a
  work driver are thin shells over it.

---

## 2. The reasoning

The design went through a full cycle: an earlier version had its own
content-addressed object store (`objects/<sha256>/`), per-node refs, append-only
status logs, version parent-chains, and edge pins inside the hashed content.
Each piece was individually defensible — and collectively they re-implemented
git inside a git repository. The current design is what remained after asking,
for each mechanism, "does git already do this?" The decisions worth recording:

### 2.1 A node is two files; git is the object store

Each node is a directory holding at most two markdown-with-TOML-frontmatter
files:

* **`node.md`** — the *definition*: what the node is and what it depends on.
* **`result.md`** — the *completion record*: what happened when the work was
  done, written once at the end of the node's single unit of work.

There is no third thing. A node with no `result.md` is open; writing
`result.md` is completing (or failing) it; editing `node.md` is revising the
definition. Adding a node only adds files, so concurrent work merges cleanly —
and the high-frequency write (recording results) touches a file nothing else
touches.

### 2.2 The node version is a git blob id, computed on demand

A node's version is `git hash-object node.md` — computed locally (the blob
hash is just `sha1("blob <len>\0" + bytes)`), never stored inside the file, and
identical to what git itself would say. Editing the definition changes the
version; writing `result.md` does not. Version history is `git log` on the
node's directory; old versions are `git show <commit>:.../node.md`.

This kills three mechanisms from the earlier design at once: the custom sha256
identity, the mutable per-node ref (git's history *is* the pointer), and the
`parent` version chain (git's commit graph is the chain). One hash authority,
zero stored hashes that must equal content.

### 2.3 Status is derived, not stored — and the enum shrank to nothing

The earlier design kept an append-only status event log so that flipping
`open → done` would not change the node's identity. The two-file split makes
the whole log unnecessary. Status is a pure function of the files:

* no `result.md` → **open**
* `outcome = "failed"` → **failed**
* `outcome = "done"` and the result's `node_version` equals the current blob id
  of `node.md` → **done**
* `outcome = "done"` but `node.md` has moved → **open again**: the completion
  certified a definition that no longer exists, so the node needs rework and
  its dependents are blocked until it gets it.

`in_progress` disappeared with the one-shot work model (nothing records "being
worked"; if parallel workers ever need claims, that belongs in the dispatcher,
not the data model). `blocked` was always derived from dependencies. What
remains — open/done/failed — is never written anywhere.

### 2.4 Pins are facts about the work, so they live in the result

`node.md` lists dependencies by **id only** (`depends_on`, `derived_from`).
Which *versions* those resolved to is recorded in `result.md`, pinned
automatically at completion time as `[[built_against]]` entries: the target's
`node.md` blob id (`pin`) and its output commit at that moment (`output`).

This placement matters. A pin is provenance — "the work saw *this*" — not part
of the task statement. If pins lived in the definition, re-pinning after an
upstream change would itself be a definition change and would falsely cascade
invalidation to *this* node's dependents. Kept in the result, updating what the
work was built against never moves the definition.

### 2.5 One output commit per node

When work produces files, `complete` commits exactly those files as **one git
commit**, and stores that commit hash on the result. A commit hash is already a
content hash of the change, so this is the node's "output hash" for free — plus
blame, diff, and history. Graph-only work (e.g. a planning node that only mints
sub-tasks) simply has no output commit.

Two derived queries fall out: `outputs <id>` reads the commit off the result;
`origin <commit>` inverts it by scanning results — unique by construction,
since each completion mints one commit for one node.

### 2.6 Staleness is derived, with explicit diffable reasons

A node with no result cannot be stale — there is no work to invalidate. For a
node with a result, each reason is checked on demand:

* **dependency definition moved** — a `built_against` pin no longer equals the
  target's current `node.md` blob id (the reason is a real diff away:
  `git diff <pin> <current>`);
* **dependency output changed** — the target was re-worked and its output
  commit differs from the pinned one;
* **context drifted** — a pinned context file's blob id no longer matches (or
  the file is gone);
* **own output drifted** — files in the output commit changed since
  (`git diff <commit>`, which also yields the explicit reason);
* **definition edited after completion** — the result no longer covers
  `node.md` (this is also what reopens the node, §2.3).

The invalidation rule of thumb: **results and status flow forward (a `done`
unblocks dependents); content changes flow backward (definitions and outputs
moving invalidate the work built on them).**

### 2.7 Readiness is derived too

A node is **ready** when it is not done and every `depends_on` target is done
(on its current definition) and not itself stale; otherwise the unsatisfied
dependencies are its **blockers**, reported with reasons. A failed node is
ready — failure means "retry", and the retry simply overwrites `result.md`.

### 2.8 Each state change is its own commit, on a clean tree

Every mutating operation commits its `.llaundry` change, and only against a
clean working tree (`complete` permits exactly the declared output files to be
dirty — it is about to commit them). So each change is its own commit, the tree
is clean between operations, and the repository state behind any change is
recoverable straight from git history. Nothing stores which commit an event
happened at; history already records it.

### 2.9 Edges are typed by the pipeline

Node types form a pipeline — task → implementation → build, with verification
attached to the artifact stages — and edges must follow it. An edge points from
later work back at what it came from, so the allowed targets per source type
are:

| from \ may link to | task | implementation | build | verification | info |
|---|---|---|---|---|---|
| task           | ✓ (sub-tasking) | | | | ✓ |
| implementation | ✓ | | ✓ (a built tool it needs) | | ✓ |
| build          | | ✓ | | | ✓ |
| verification   | | ✓ | ✓ | | ✓ |
| info           | ✓ | ✓ | ✓ | ✓ | ✓ |

`info` is freeform documentation, linkable in both directions. The rules are
enforced at the only two edge-creation points (`add`, `link`) by
`NodeType::allowed_targets`, so an ill-typed graph cannot be constructed *by
the tools*; both `depends_on` and `derived_from` follow the same table.

Write-time validation cannot see damage that enters sideways — hand edits, or
a git merge combining two individually-valid branches. The complement is
`llaundry check` (and the `check_store` MCP tool): an fsck-style scan that
re-derives every invariant over the store as it actually is — files parse,
edge targets exist, type rules hold, no duplicates or self-references, and no
`depends_on` cycles (which would deadlock readiness). Like every other query
it is derived, read-only, and git-free.

### 2.10 What context is: outputs first, files second

Most of what work consumes is *other nodes' outputs* — covered by the
`built_against` output pins, one hash per dependency. Explicit per-file
`[[context]]` pins (blob ids) exist for the remainder: pre-existing files that
no node produced. They are expected to be the minority case.

---

## 3. On-disk layout

A store is a single directory (default `.llaundry/`, committed to git):

```text
.llaundry/
  nodes/
    <id>/
      node.md      # the definition
      result.md    # the completion record (absent until worked)
```

### 3.1 `node.md`

```markdown
---
schema = 1
type = "task"
title = "Parse the config file"
author = "human"
depends_on = ["task-01J8XQ2A..."]
derived_from = ["info-01J8XQ1B..."]
---

Parse the TOML config into the Config struct...
```

The frontmatter is strict, typed TOML (scalars and string arrays only); the
body is free-form Markdown. The file's git blob id is the node's version.

### 3.2 `result.md`

```markdown
---
at = 1719571200000
author = "machine"
node_version = "4ec1916e..."     # blob id of node.md this work fulfilled
outcome = "done"                  # or "failed"
output_commit = "a45ab51c..."     # the one commit with everything produced; optional

[[built_against]]
id = "task-01J8XQ2A..."
pin = "6102d492..."               # target's node.md blob at completion
output = "86cb1a1..."             # target's output commit at completion; optional

[[context]]
path = "docs/legacy-format.txt"
blob = "f44d..."
---

Implemented the parser in src/config.rs. Chose serde over hand-rolling because...
```

The body is the narrative — for an LLM worker, the story of what it did and
why. (`at` is Unix milliseconds — deliberately dependency-free.)

### 3.3 Ids

`<type-prefix>-<ULID>`, e.g. `task-01J8XQ3K7M...`. The prefix is
human-scannable; the ULID is time-sortable and collision-free without a central
counter, so nodes can be minted concurrently. Uniqueness is enforced by the
filesystem (the directory either exists or it doesn't).

---

## 4. Relationship to git

llaundry stores nothing git could store for it. Git owns content integrity,
immutable history, blame, authorship, branching, merge, and distribution. The
only thing llaundry adds — the part git cannot express — is the typed-graph
semantics: dependencies, derived status, pins, and staleness. That semantic
layer is the product; everything storage-shaped is delegated.

The CLI drives git (§2.8): every mutating command checks the tree is clean and
commits its own store change, so all mutating commands require a git repository
with at least one commit and a configured identity. The read-only queries need
no git at all — blob hashing is computed locally (and verified against
`git hash-object` in tests) — except `log`, which *is* git log.

To keep the git dependency out of tests, the remaining git interaction goes
through a small `Vcs` trait (`capture`, `commit_store`, `drift`,
`dirty_paths`). The real `GitVcs` shells out to `git`; unit tests inject an
in-memory `FakeVcs`, so the store, derived status, staleness, blockers, and the
complete/fail flows run with no git binary, repository, or identity.

---

## 5. The frontends

All functionality lives in the `llaundry` library (`llaundry::ops` over
`llaundry::store`, with git behind the `Vcs` seam). Every executable is a thin
shell over it:

* `llaundry` — the CLI (below).
* `llaundry-tui` — an interactive terminal UI over the same operations.
* `llaundry-mcp` — a Model Context Protocol server (§5.2).
* `llaundry-work` — the driver that runs an LLM against a node (§5.3).

### 5.1 The CLI

The store path defaults to `.llaundry/`, overridable with `--store` or
`LLAUNDRY_DIR`.

| Command | Purpose |
|---|---|
| `init` | Create an empty store. |
| `add` | Create a node (`--depends-on`/`--derived-from` by id). Prints its id. |
| `link <from> <to>` | Add a dependency (a definition change of `<from>`). |
| `edit <id>` | Change title/body (a definition change: reopens a done node). |
| `complete <id> [-o <file>...] [--notes ...]` | Commit produced files as one output commit, pin deps/context, write `result.md`. |
| `fail <id> [--notes ...]` | Record a failed attempt. |
| `show <id>` | Definition, derived status, result, staleness reasons. |
| `list` | Every node with its derived status. |
| `log <id>` | The node's git history (definition edits and results). |
| `stale` | Nodes whose recorded work has been invalidated, with reasons. |
| `ready` / `blocked` | Derived readiness, with blocker reasons. |
| `outputs <id>` / `origin <commit>` | Provenance in both directions. |
| `dependents <id>` | Which nodes depend on / derive from this one. |
| `check` | Integrity-check the store (fsck): parse errors, missing/ill-typed edge targets, duplicates, self-references, dependency cycles. Non-zero exit on problems. |

### Example

```sh
llaundry init

A=$(llaundry add --title "Define config schema")
B=$(llaundry add --title "Parse config file" --depends-on "$A")

llaundry blocked            # B waits on A
llaundry complete "$A" --notes "schema agreed"
llaundry ready              # -> B

echo 'fn parse() {}' > src/config.rs
llaundry complete "$B" -o src/config.rs --notes "implemented parser"

# Revising A reopens it and flags B, with the pinned-vs-current versions:
llaundry edit "$A" --title "Define and validate config schema"
llaundry stale
#   A: definition changed since the work (...)
#   B: dependency A: definition moved (built against 6102d4, now 516cc0)

# Provenance both ways:
C=$(llaundry outputs "$B"); llaundry origin "$C"    # -> B
llaundry dependents "$A"                             # -> B
```

### 5.2 The MCP server

`llaundry-mcp` exposes the same operations over the
[Model Context Protocol](https://modelcontextprotocol.io) (JSON-RPC 2.0 on
stdio, synchronous, dependency-light). Each tool is a type implementing a small
`Tool` trait listed in a `registry()`; every `call` is a thin wrapper over one
`ops::*` function.

Tools: `init_store`; `add_node`, `link_nodes`, `edit_node`, `complete_node`,
`fail_node` (mutating); `show_node`, `list_nodes` (reads); `stale_nodes`,
`ready_nodes`, `blocked_nodes` (derived queries); `node_origin`,
`node_outputs`, `node_dependents` (provenance). The mutating tools inherit the
clean-tree rule and surface refusals as in-band MCP errors (`isError: true`).

### 5.3 The worker

`llaundry-work` runs one unit of work on one node. It refuses to start if the
node is blocked (override with `--force`), builds a prompt from the node's
type, title, body, and dependency ids, and hands it to a `Backend`. The first
backend, `ClaudeCode`, shells out to `claude -p` sandboxed to **only** the
llaundry MCP server (`--strict-mcp-config`, `--allowedTools mcp__llaundry`) —
no shell, file, or network tools. A file-free session produces no output
commit, so the prompt steers it to finish with `complete_node` (notes as the
record of what happened) or `fail_node`. Command construction is separated from
execution so the exact invocation is unit-tested and shown by `--dry-run`.

---

## 6. Deliberately out of scope (for now)

* **Context enforcement.** Consumed context is recorded and pinned, but nothing
  prevents an agent from reading undeclared files — a runtime concern for the
  MCP server.
* **Transitive staleness.** Each node's own pins are checked; a node is not
  auto-flagged because something upstream of *its dependency* moved. Output and
  context pins reduce the need: consumers are flagged directly when what they
  actually consumed changes.
* **Executing builds and verifications.** Nodes can be typed `build` /
  `verification`, but running them is not implemented.
* **Re-certification without rework.** When a dependency moves, the only way to
  clear the staleness today is to re-complete the node. A cheap `repin` ("I
  inspected the diff; my work still stands") would be a small addition.
* **Claiming for parallel workers.** With no `in_progress`, two dispatchers
  could hand the same open node to two workers; coordination belongs in the
  dispatcher.
