# llaundry — design

This document describes llaundry's data model, the reasoning behind it, and the
tools that operate on it. It is a record of the design discussion as much as a
specification.

llaundry's premise (see `README.md` / `ideas.wiki`): **the prompt history is the
story, not the output code.** Work is driven by a graph of nodes, each carrying
the description that drives the work and, once worked, a record of what
happened: the context consumed, the output produced, and the narrative. The two questions the system
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

A node with no `result.md` is open; writing `result.md` is completing (or
failing) it; editing `node.md` is revising the definition. Adding a node only
adds files, so concurrent work merges cleanly — and the high-frequency write
(recording results) touches a file nothing else touches.

There is one more file a node may carry — `work.jsonl`, the recorded
interaction log of work sessions (§2.11) — but it is deliberately *not* a third
kind of state: it is opaque to every derived query, participates in nothing,
and exists purely as a record.

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

### 2.8 Each state change is its own commit; cleanliness where provenance is asserted

The store lives in a *workbench*: an outer git repository holding `.llaundry/`
next to `project/`, the actual project — an ordinary, completely separate git
repository (see ISOLATION.md for the layout and the isolation reasoning).

Every mutating operation commits its `.llaundry` change to the workbench
repository, so each graph change is its own commit there and the store's
history is a linear journal — branching the project does not fork the
database. The project repository is checked only where output provenance is
asserted: `complete` refuses undeclared dirty writes (it permits exactly the
declared output files — it is about to commit them). Pure graph edits (add,
link, edit, fail) never gate on project state: jotting nodes mid-hack is
fine, and recording a failure is possible even when the failed attempt left
a mess. Nothing stores which commit an event happened at; history already
records it.

### 2.9 One node type

Earlier iterations typed every node (task / implementation / build /
verification) and constrained which types could link to which, encoding the
intended pipeline as write-time edge rules. That machinery was removed: none of
the core mechanics — status derivation, pins, staleness, readiness, provenance
— ever branched on the type, so the taxonomy was a schema bolted onto the
graph, maintained before real usage had shown which distinctions matter. Today
there is a single node kind; what a node *is* lives in its description (whose
first line serves as its title). A
taxonomy (and per-type behaviour, e.g. runnable builds) can be reintroduced
once usage of the tool makes the right shape clear.

The same reasoning already removed the freeform "info" type: knowledge is
modelled as work that produced it. An originating request is a *root node*
(sub-nodes derive from it, and revising the request flags them all); a decision
or research finding is a node whose result notes record the outcome. Every
node is therefore workable, and prose context lives in exactly two places — a
node's description and its result notes.

Edge creation (`add`, `link`) validates that the target exists, rejects
self-references and duplicates, and nothing more.

Write-time validation cannot see damage that enters sideways — hand edits, or
a git merge combining two individually-valid branches. The complement is
`llaundry check` (and the `check_store` MCP tool): an fsck-style scan that
re-derives every invariant over the store as it actually is — files parse,
edge targets exist, no duplicates or self-references, and no `depends_on`
cycles (which would deadlock readiness). Like every other query it is derived,
read-only, and git-free.

### 2.10 Questions are nodes; `assignee` says who a node is for

An agent mid-work that needs a human decision must not fail (that means "the
work cannot be done") and must not silently stall. The design's own move from
§2.9 — knowledge is work that produced it — extends to questions: **a question
is a unit of work assigned to a human, and the answer is its result.** The
agent adds a question node (`assignee = "human"`, the context and options in
its description), links its own node to `depends_on` it, and stops without
completing. Its node is now derived-*blocked*, not failed; the question shows
up in `ready --for human` — the human's inbox; the human completes it with the
answer as result notes; the asker becomes ready again.

Everything else falls out of existing machinery: the answer is a dependency,
so it is pinned at completion — a human who later *revises* an answer makes
the work built on it stale, with a diffable reason. No new node kind, no
message channel, no state machine.

`assignee` is the one addition: an optional scalar on the definition saying
who the work is *for* (`human`/`machine`), distinct from `author` — a
machine-authored question is human-assigned. Absent means anyone may work it.
Dispatch respects it (`llaundry-work` refuses a human-assigned node; `ready`
filters on it), but no derived semantics — status, staleness, readiness —
branch on it.

### 2.11 The interaction log: the story is recorded, not remembered

The premise is that the prompt history is the story — so the story is
recorded mechanically, not left to agent discipline. Every `llaundry-work`
session's full interaction stream (one JSON event per line: the prompts, the
assistant turns, every tool call and result) is *streamed* to the node's
`work.jsonl` as it happens — one flushed line per event, opened at launch with
a small attempt header (timestamp, backend and model, the `node.md` version
the session set out to work). Streaming, not buffering, is the point: an abrupt end
(Ctrl-C, crash, kill) loses at most an unflushed tail, never the story so far,
so no exit — however rude — leaves the node without its record.

The log is what makes the pause-on-a-question protocol (§2.10) resumable:
when a node that paused mid-unit (open, no result, log present) is worked
again, the previous log is replayed verbatim into the new session, which
continues exactly where the last one stopped — the final events of a paused
log *are* the question being minted. There is deliberately no backend-native
session resume: the log in git is the only continuation mechanism, so it
works on any clone, after any delay, with any backend.

Three rules keep it honest:

* **Opaque.** No derived query reads it; it does not participate in the node
  version; writing it reopens and stales nothing. It is narrative,
  machine-grade instead of prose-grade.
* **Streamed, swept, never blocking.** A streaming log is dirty for the whole
  session — but only in the workbench repository, which is entirely
  machine-written, so it gates nothing (the project repo's cleanliness rule,
  §2.8, is untouched by it). Every store commit the session makes (`git add`
  on the store directory) sweeps the log written up to that moment, giving
  incremental durability in git for free — a commit that sweeps half a story
  in is still a true story-so-far; the driver commits the remaining tail when
  the session ends. A crash between commits leaves a dirty log that blocks
  nothing and is swept in by whatever store commit comes next.
* **Appended for continuation, restarted for rework.** A paused unit of work
  extends its log (appends diff minimally — goal #1); a node being reworked
  after a recorded result starts a fresh story, and git history keeps the old
  one — exactly the `result.md` overwrite semantics.

### 2.12 What context is: outputs first, files second

Most of what work consumes is *other nodes' outputs* — covered by the
`built_against` output pins, one hash per dependency. Explicit per-file
`[[context]]` pins (blob ids) exist for the remainder: pre-existing files that
no node produced. They are expected to be the minority case.

Context pins come from two sources. The worker declares them at `complete`;
then, because every work session is recorded verbatim (`work.jsonl`), the
driver mines the transcript afterwards and pins any project file the worker
was *observed* reading but did not declare — marked `observed = true` to keep
self-reported and derived provenance distinguishable. Input provenance is thus
a derived fact, not agent discipline, matching how output provenance is
enforced (the clean-tree rule refuses `complete` while undeclared writes are
dirty, whatever tool wrote them).

---

## 3. On-disk layout

A store is a single directory (default `.llaundry/`) inside a workbench,
beside the project it describes (§2.8, ISOLATION.md):

```text
<workbench>/       # outer git repo: the store's history
  .llaundry/
    nodes/
      <id>/
        node.md      # the definition
        result.md    # the completion record (absent until worked)
        work.jsonl   # the recorded interaction log (absent until worked by the driver)
  project/         # inner git repo: the actual project, ordinary in every way
```

### 3.1 `node.md`

```markdown
---
schema = 1
author = "human"
assignee = "human"                # optional: who the work is for (§2.10)
depends_on = ["node-01J8XQ2A..."]
derived_from = ["node-01J8XQ1B..."]
---

Parse the config file

Parse the TOML config into the Config struct...
```

The frontmatter is strict, typed TOML (scalars and string arrays only); the
body is the node's description, free-form Markdown. There is no stored title:
the description's first line serves as the title wherever a one-liner is
needed. The file's git blob id is the node's version.

### 3.2 `result.md`

```markdown
---
at = 1719571200000
author = "machine"
node_version = "4ec1916e..."     # blob id of node.md this work fulfilled
outcome = "done"                  # or "failed"
output_commit = "a45ab51c..."     # the one commit with everything produced; optional

[worked_by]                       # the engine that did the work; optional
backend = "claude-code"           # stamped by the driver after the session
model = "opus"                    # absent = the backend's default at the time

[[built_against]]
id = "node-01J8XQ2A..."
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

### 3.3 `work.jsonl`

The interaction log (§2.11): one JSON object per line, exactly as the backend
streamed it, each attempt preceded by a header line the driver stamps at
launch:

```jsonl
{"event":"attempt","at":1719571200000,"backend":"claude-code","model":"opus","node_version":"4ec1916e..."}
{"type":"system","subtype":"init",...}
{"type":"assistant","message":{...}}
{"type":"user","message":{...}}
...
```

No schema of llaundry's own beyond the header — the event lines are whatever
the backend emits, kept verbatim so replay is lossless.

### 3.4 Ids

`node-<ULID>`, e.g. `node-01J8XQ3K7M...`. The ULID is time-sortable and
collision-free without a central counter, so nodes can be minted concurrently. Uniqueness is enforced by the
filesystem (the directory either exists or it doesn't).

---

## 4. Relationship to git

llaundry stores nothing git could store for it. Git owns content integrity,
immutable history, blame, authorship, branching, merge, and distribution. The
only thing llaundry adds — the part git cannot express — is the graph
semantics: dependencies, derived status, pins, and staleness. That semantic
layer is the product; everything storage-shaped is delegated.

The CLI drives git (§2.8) over the workbench's two repositories: every
mutating command commits its own store change to the workbench repo, and
`complete` additionally checks and commits the project repo — so mutating
commands require git repositories with a configured identity (`init` creates
both). The read-only queries need no git at all — blob hashing is computed
locally (and verified against `git hash-object` in tests) — except `log`,
which *is* git log on the workbench repo.

To keep the git dependency out of tests, the remaining git interaction goes
through a small `Vcs` trait whose methods split along the two repositories:
`commit_store` speaks to the workbench repo; `capture`, `drift`, `files_in`,
and `dirty_paths` to the project repo. The real `GitVcs` shells out to `git`;
unit tests inject an in-memory `FakeVcs`, so the store, derived status,
staleness, blockers, and the complete/fail flows run with no git binary,
repository, or identity.

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
| `init` | Create a workbench: the store, the project directory, and a git repository for each. |
| `add` | Create a node (`--depends-on`/`--derived-from` by id, `--assignee` for who the work is for). Prints its id. |
| `link <from> <to>` | Add a dependency (a definition change of `<from>`). |
| `edit <id>` | Change the description (a definition change: reopens a done node). |
| `complete <id> [-o <file>...] [--notes ...]` | Commit produced files as one output commit, pin deps/context, write `result.md`. |
| `fail <id> [--notes ...]` | Record a failed attempt. |
| `show <id>` | Definition, derived status, result, staleness reasons. |
| `list` | Every node with its derived status. |
| `log <id>` | The node's git history (definition edits and results). |
| `stale` | Nodes whose recorded work has been invalidated, with reasons. |
| `ready` / `blocked` | Derived readiness, with blocker reasons. `ready --for human` is the human's inbox of pending questions (unassigned nodes match either). |
| `outputs <id>` / `origin <commit>` | Provenance in both directions. |
| `dependents <id>` | Which nodes depend on / derive from this one. |
| `check` | Integrity-check the store (fsck): parse errors, missing edge targets, duplicates, self-references, dependency cycles. Non-zero exit on problems. |
| `settled <id>` | Whether the node *and all work transitively derived from it* is done and not stale — "is this branch actually finished?" (a node's own `done` only certifies its own unit of work, e.g. a task that closed at spec time). Non-zero exit if not. |

### Example

```sh
llaundry init

A=$(llaundry add --description "Define config schema")
B=$(llaundry add --description "Parse config file" --depends-on "$A")

llaundry blocked            # B waits on A
llaundry complete "$A" --notes "schema agreed"
llaundry ready              # -> B

echo 'fn parse() {}' > src/config.rs
llaundry complete "$B" -o src/config.rs --notes "implemented parser"

# Revising A reopens it and flags B, with the pinned-vs-current versions:
llaundry edit "$A" --description "Define and validate config schema"
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
node is blocked or assigned to a human (override with `--force`), builds a
prompt from the node's description and dependency ids, and hands it to a
`Backend`. The first backend, `ClaudeCode`, shells out to `claude -p` with
its working directory pinned to the workbench's `project/` and a whitelist
grant: the llaundry MCP server (`--strict-mcp-config`) plus the file tools,
every one scoped to the working directory (`Read(./**)` … `Write(./**)`) —
no shell, no network tools (web only behind `--network`). The store and its
history sit above the granted subtree, so no rule about them exists at all
(ISOLATION.md). The prompt steers the session to
finish with `complete_node` (notes as the record of what happened) or
`fail_node` — or, when it needs a human decision, to pause: mint a
human-assigned question node, depend on it, and stop (§2.10). Command
construction is separated from execution so the exact invocation is
unit-tested and shown by `--dry-run`.

Which backend, model, and executables the driver uses default to the store's
optional `config.toml` (`<store>/config.toml`, versioned with the rest of the
store), so a workbench pins its choices once instead of respelling them on
every invocation:

```toml
[work]
backend = "claude-code"   # default backend when --backend is not given
mcp-bin = "llaundry-mcp"  # the MCP server binary the model may use

[work.claude-code]        # per-backend settings, keyed by backend name
model = "opus"            # model to request (backend default if unset)
bin   = "claude"          # the Claude Code executable
```

Every field is optional and layered: an explicit `--flag` wins, else the file,
else the built-in default. A missing file means all-defaults; a present but
malformed one is a hard error, so a typo surfaces rather than being ignored.

The driver streams every session's interaction events
(`--output-format stream-json`, teed to the terminal) to the node's
`work.jsonl` as they arrive, flushed per line — so an interrupted or crashed
session keeps its story (§2.11) — and commits whatever tail the session's own
store commits did not already sweep in when the backend exits, successfully
or not. On launching a node that is open with no result but with a recorded
log — a paused unit of work — it replays that log into the prompt so the new
session continues where the previous one stopped.

Which engine did the work is recorded mechanically, like observed context:
after the session, the driver stamps the resolved backend and model onto the
result's `worked_by` (guarded by the attempt timestamp, so a rework session
that died without writing a new result cannot mislabel the old one). The
worker itself is never asked to know what it runs on. Results recorded by
hand carry no stamp.

---

## 6. Deliberately out of scope (for now)

* **Context enforcement within the project.** The workbench layout confines a
  session to the project tree and keeps the graph reachable only through the
  MCP server (ISOLATION.md), but *within* the project, reads are observed and
  pinned rather than pre-authorised. Narrowing a session's view to a node's
  declared context is the natural end-state; not built.
* **Transitive staleness.** Each node's own pins are checked; a node is not
  auto-flagged because something upstream of *its dependency* moved. Output and
  context pins reduce the need: consumers are flagged directly when what they
  actually consumed changes.
* **Node taxonomy and executable stages.** Earlier designs typed nodes (task /
  implementation / build / verification) with per-type edge rules; the types
  were removed until usage shows which distinctions matter (§2.9). Running
  builds or verifications would come back with them.
* **Re-certification without rework.** When a dependency moves, the only way to
  clear the staleness today is to re-complete the node. A cheap `repin` ("I
  inspected the diff; my work still stands") would be a small addition.
* **Claiming for parallel workers.** With no `in_progress`, two dispatchers
  could hand the same open node to two workers; coordination belongs in the
  dispatcher.
