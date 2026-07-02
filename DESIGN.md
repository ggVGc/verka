# llaundry — database design

This document describes the local database that backs llaundry, the reasoning
behind it, and the small CLI tool that operates on it. It is a record of the
design discussion as much as a specification.

llaundry's premise (see `README.md` / `ideas.wiki`): **the prompt history is the
story, not the output code.** Work is driven by a graph of nodes — tasks,
implementations, builds, verifications — each carrying the prompt and
context that produced it. The output can in principle be regenerated from that
graph. To make that credible the store has to be *verifiable* (content-addressed)
and *auditable* (immutable history, clear human/machine authorship). This is "git
for LLM development".

---

## 1. Goals

* **Text-based and diff-friendly.** Every record is human-readable text, and any
  change produces a minimal, reviewable diff.
* **Lives in git.** Git is the storage, history, integrity, and (eventually)
  collaboration substrate. We do not build a storage layer on top of it.
* **Immutable and content-addressed.** A node's identity *is* its content. Editing
  anything yields a new identity; nothing is rewritten in place.
* **Verifiable dependencies.** A node records which exact versions of other nodes
  it was built against, so we can detect when an upstream change has invalidated
  downstream work.
* **Minimal.** The core is just the database and its operations, exposed as a
  library. Frontends stay thin: a CLI, a TUI, and an MCP server all wrap the same
  `llaundry::ops`. No build/verification execution.

---

## 2. The reasoning (summary of the discussion)

The design fell out of a sequence of decisions, each worth recording because the
*why* matters more than the *what*.

### 2.1 One file (record) per node, not one aggregate file

A single aggregate file (JSON/JSONL/SQLite dump) makes every write touch the same
bytes: concurrent agents conflict, and edits produce noisy diffs. With one record
per node, **adding a node is purely additive** — you write new files and never
touch existing ones — which mirrors how git itself grows (a commit points at its
parents; it does not rewrite them). Diffs stay surgical and parallel work merges
cleanly.

### 2.2 Structured data is strict; prose is loose

Markdown front-matter is too loose for the structured fields (it is schemaless and
has type-coercion traps). So we split the two concerns:

* **`meta.toml`** holds the strict, typed, schema-checkable fields.
* **`body.md`** holds the prose (the prompt / task statement / discussion), where
  looseness is appropriate and Markdown tooling and line-by-line diffs are wanted.

TOML (over JSON) for the structured part because it diffs better — one key per
line, no brace/comma churn — and because keeping the prose in its own `.md` file
avoids escaping newlines into a single string.

### 2.3 No persisted index

A shared `index.json` aggregating all nodes would reintroduce exactly the
contention that per-node files removed: committed, it causes merge conflicts;
derived, it races and goes stale; either way it duplicates the source of truth.

The resolution mirrors how **git treats its own index**: the index is a
*rebuildable cache*, never the authority. Git's authoritative data — the object
store — avoids contention entirely through content-addressing and immutability
(idempotent writes, never in place), and the only mutable thing (refs) is tiny and
guarded by lock + atomic rename. So: **no persisted index.** The directory of
records *is* the index; a reader rebuilds whatever lookup structures it needs in
memory by scanning. (This CLI scans on each invocation, which is trivially fast at
realistic sizes; a long-running process would scan once and keep it in memory.)

### 2.4 One hash, not two

Both git and llaundry want a content hash. Keeping two authoritative hashes in
sync is a problem you should simply not have. The fix is to never *store* a hash
that must equal the content:

* The node's hash is computed from its bytes (here, `sha256` of the record). It is
  **not** stored inside the record — the location of the record is its only name —
  which also avoids the self-reference problem (hashing a file that contains its
  own hash).
* If you instead adopt git's own object id as the node hash, there is likewise
  only one hash. Either way: one authoritative hash, recomputed, never duplicated.

### 2.5 Everything immutable — and the one irreducible mutable pointer

The strongest position, and the one we took: **updating any property of a node
changes its identity.** There are no in-place edits. But immutability does not
*remove* mutable state — it *relocates and shrinks* it. Git proves this: its
objects are 100% immutable, yet `refs/heads/main` must move, because *something*
has to answer "which immutable object is current," and that answer changes over
time.

So any system with a notion of "the current state" needs exactly one kind of
mutable thing: a pointer to the latest immutable version. We shrink it to the
smallest possible surface — **one ref per logical node** — and make everything
else immutable.

### 2.6 Status is an event, not a field

If status were a field of the node, flipping `open -> done` would change the
node's hash and thereby **falsely invalidate every dependent** (they pinned the
old hash, see §2.7) — even though the *definition* did not change. So status is
modelled as an **append-only log of immutable events** kept *outside* the hashed
content. A status change appends an event and never alters any node's identity.
Each kind of change re-hashes only its own object, which is exactly what
content-addressing should do.

Status is a small, closed, **validated enum** — `open | in_progress | done |
failed` — like `type` and `author`. Notably there is **no `blocked`**: whether a
node is blocked is a fact about its *dependencies*, which the graph already records
as edges, so it is *derived* (see §2.10), never stored. A stored `blocked` flag
would only duplicate the graph and then drift — nothing would clear it when the
blocking dependency finished — the same store-vs-derive anti-pattern avoided for the
index (§2.3) and hashes (§2.4).

### 2.7 Edges carry a logical id *and* a pinned version

An edge stores both:

* the **logical id** of the target — a stable handle, so "depends on task X"
  survives edits to X; and
* the target's **version hash at link time** (the *pin*) — so we can tell when X
  has moved on.

If the target later gets a new version, its ref no longer equals the pin, and the
dependent is **stale**: it was built against a definition that has since changed.
This is the mechanism behind the "edit a test -> graph partially invalidated ->
agents rework the affected nodes" workflow.

### 2.8 Produced outputs are a git commit, not a hash we compute

When a node is completed by producing files (e.g. source code), we do **not** hash
those files ourselves — that would duplicate what git already does. Instead:

1. The produced files are **committed with git**. That commit captures the exact
   diff, and its hash *is* a content hash of the change.
2. That **commit hash is stored on the node** as its output reference (and, being
   part of `meta.toml`, becomes part of the node's own identity hash).
3. The store change (the new node version) is then committed too.

This gives the same two properties as before, but for free from git:

* **A verifiable claim**: "this version produced exactly the diff in commit `C`,"
  captured alongside the prompt and context that produced it.
* **Drift detection**: staleness is simply *"have any of the files that commit `C`
  touched changed since `C`?"* — answered by `git diff C`, which also yields the
  **explicit reason** (a real `name-status` / diff), strictly better than a
  hash-mismatch. Same staleness machinery as edges (§2.7), delegated to git.

This is the cleanest expression of the project's principle (§2.3, §2.4): git owns
content integrity and diffs; llaundry only records *which commit* is the output and
*which node* it belongs to — the semantics git cannot express. The trade-off is
that `complete` now requires a git repository (see §4).

### 2.9 Inputs and used context are pinned by content too

An agent works a node using only what is *declared* on it: the connected nodes
(edges) and a set of **declared input files**. Because that context is declared up
front and treated as a closed sandbox — not discovered after the fact — it is fully
knowable and can be pinned by content hash, exactly like outputs. So:

* **Declared inputs** (`inputs`) are pinned at `add` time. If a declared input
  later changes, the node is stale — and, crucially, the *consumer* is flagged
  directly (it pinned the actual content), without waiting for the producing node
  to be re-versioned. This closes the gap where a raw file edit only flagged the
  producer (§2.8) and not its dependents.
* **Recorded context** (`context`) covers what was *actually* used during the work
  but wasn't pre-declared — e.g. files a coding agent's tool calls read. It is
  pinned at `complete` time. It is provenance plus a staleness source: if that
  context later changes, the node is flagged too.

The principle behind "only what is declared": if an agent needs more than its
declared context, that is not a licence to read arbitrary files — it is a signal to
create a **new node** (e.g. a task "search the web for X") that *produces an
output*, which is then wired in as an input to the downstream work. The graph stays
closed, and every input remains a tracked, content-addressed thing.

Inputs and context pin git **blob ids** (`git hash-object`), not commits: they
reference existing content a node consumed, rather than a change it made, so no new
commit is involved. (*Enforcement* — sandboxing an agent so it physically cannot
read undeclared context — is a runtime/MCP concern and out of scope here; recording
the pins is useful for invalidation regardless.)

### 2.10 Readiness is derived, not stored

Because dependencies are explicit edges and every node carries its status, "can
this node be worked yet?" is a query, not a stored flag. A node is **ready** when
every `depends_on` target is `done` and not itself stale, and **blocked** otherwise
— with the unsatisfied dependencies as the explicit reason. The `ready` and
`blocked` commands compute this each time, so it can never disagree with the graph.
This is why §2.6 drops `blocked` from the status enum: it belongs here, derived.

### 2.11 Each state change is its own commit, on a clean tree

Every mutating operation commits its `.llaundry` change, and only against a clean
working tree. So each state change is its own commit, the tree is clean between
operations, and **the repository state behind any change is recoverable straight from
git history**: `git blame status/<id>.toml` maps each event to the commit that
recorded it, whose parent is the baseline the change was made against (for `done`,
that baseline is the output commit, already on the node as `output_commit`).

For that to hold, the tree must be clean when a change is recorded — otherwise
uncommitted changes wouldn't be captured by the commit. So the tool **enforces a
clean tree**: `add`, `set-status`, `link`, and `edit` refuse to run against a dirty
tree, and `complete` permits only the declared output files to be dirty (it is about
to commit exactly those).

We deliberately do **not** store the commit on the event itself — that would
duplicate what history already records, and a stored hash could dangle if the
history were rewritten. Same store-vs-derive principle as the index (§2.3) and
`blocked` (§2.10). The enforcement plus the per-change commit are what make history a
faithful record — the "use git as the graph" stance (`ideas.wiki`). (Consequently
all mutating commands require a git repository with at least one commit.)

### 2.12 A `done` status certifies a specific version

Each status event stores the node **`version`** (content hash) it was asserted
against. This matters for completion: marking a node `done` certifies *that
version's* content. An edit produces a new version without touching the status log,
so a node completed and then edited would still show `done` — yet the current version
was never completed.

So a `done` is treated as **stale when the node has moved past the version it was set
on** (reported as `done on an older version` by `stale`/`show`). And because
dependency satisfaction routes through the same staleness check, `ready`/`blocked`
stop counting such a dependency as done: a consumer of a node that was completed and
then edited is blocked again until the node is re-completed on its current version.

`open` and `in_progress` don't certify content, so they are *not* version-sensitive;
only the completion claim is. (The same reasoning would extend to `failed`.) This is
the counterpart, for a node's own lifecycle, of the edge/input/output staleness in
§2.7–2.9 — the `version` on each event is what makes it computable.

---

## 3. On-disk layout

A store is a single directory (default `.llaundry/`, committed to git):

```text
.llaundry/
  objects/
    <hash>/
      meta.toml        # immutable definition of one node version
      body.md          # immutable prose for that version
  refs/
    <logical-id>       # one line: the current version hash  (the only mutable file)
  status/
    <logical-id>.toml  # append-only [[event]] log
```

Three categories, mapping directly onto the git model:

| Area       | Mutability   | git analogue         |
|------------|--------------|----------------------|
| `objects/` | immutable    | the object store     |
| `refs/`    | mutable      | `refs/heads/*`       |
| `status/`  | append-only  | (an event log)       |

### 3.1 Object: `objects/<hash>/meta.toml`

```toml
schema = 1
logical_id = "task-01J8XQ3K7M..."
type = "task"
title = "Parse the config file"
author = "human"
parent = "9f1c..."          # previous version hash; omitted on the first version
output_commit = "86cb1a1..." # git commit capturing this version's outputs; omitted until completed

[[edges]]
to = "task-01J8XQ2A..."
rel = "derived_from"
pin = "4a7e..."

[[edges]]
to = "task-01J8XQ4P..."
rel = "depends_on"
pin = "1b2c..."

[[inputs]]                  # declared input files, pinned by content (git blob id)
path = "src/config.rs"
content = "ebb1..."

[[context]]                 # context actually used during work (e.g. a tool-call read)
path = "src/helper.rs"
content = "f44d..."
```

`body.md` is free-form Markdown. Scalar keys (including `output_commit`) precede
the `[[edges]]`/`[[inputs]]`/`[[context]]` arrays-of-tables, as TOML requires.

**Hash.** `hash = sha256(meta.toml bytes || 0x00 || body.md bytes)`, hex-encoded.
It is computed over the exact bytes written, and is never stored inside the record.
A given (meta, body) pair therefore always lands at the same path — writes are
idempotent, and identical content is deduplicated automatically.

**Identity includes everything definitional**: `logical_id`, `type`, `title`,
`parent`, `output_commit`, `edges`, `inputs`, `context`, and the body. Change any of
them and you get a new hash, i.e. a new version. `parent` links versions into a
history chain.

### 3.2 Ref: `refs/<logical-id>`

A one-line text file containing the current version hash. This is the single
mutable element of the whole store. "Editing" a node means: write a new immutable
object, then point the ref at it.

### 3.3 Status log: `status/<logical-id>.toml`

```toml
[[event]]
at = 1719571200000
status = "open"
author = "human"
version = "9f1c..."

[[event]]
at = 1719574800000
status = "done"
author = "machine"
version = "9f1c..."
```

Appended to, never edited. Appending another `[[event]]` block keeps the file
valid TOML while adding only new lines. The current status is the last event. Each
event's `version` is the node-version hash the status was asserted against — a `done`
certifies only that version (§2.12). The event does *not* store which commit it
happened at; that is recoverable from git history (§2.11), since every change is its
own commit. (`at` is Unix milliseconds — deliberately dependency-free; a future
version may switch to RFC 3339.)

### 3.4 Logical ids

`<type-prefix>-<ULID>`, e.g. `task-01J8XQ3K7M...`. The prefix is human-scannable;
the ULID is time-sortable and collision-free without any central counter, which
matters because nodes may be minted concurrently. (A sequential counter would be a
shared mutable hotspot — the very thing we are avoiding.)

---

## 4. Relationship to git

llaundry stores nothing git could store for it. Git owns: content integrity,
immutable history, blame, authorship/signing, branching, merge, and distribution.
The records are plain text in a committed directory, so all of that applies for
free. The only thing llaundry adds — the part git cannot express — is the
**typed-graph semantics**: typed edges, node status, version chains, and staleness.
That semantic layer is the product; everything storage-shaped is delegated to git.

Concretely: commit the `.llaundry/` directory like any other source. Because each
node lives in its own immutable file, history and merges are clean by construction.

The CLI goes further and *drives* git (§2.11): every mutating command checks the
working tree is clean (`complete` allows only its declared outputs), records the
relevant commit, and commits its own store change. So all mutating commands require
a git repository with at least one commit and a configured identity; the read-only
commands do not.

To keep that git dependency from leaking into tests, all git interaction goes
through a small `Vcs` trait (`capture`, `commit_store`, `drift`, `content_id`,
`dirty_paths`). The real implementation (`GitVcs`) shells out to `git`; the
command logic takes `&dyn Vcs`. Unit tests inject an in-memory `FakeVcs`, so the
store, hashing, edge/input/context/output staleness, the clean-tree checks, and the
`complete` flow are all exercised with **no git binary, no repository, and no
configured identity** — fast, deterministic, self-standing. A separate (optional)
integration test can exercise real `GitVcs`.

---

## 5. The frontends

All functionality lives in the `llaundry` **library** (`llaundry::ops` over
`llaundry::store`, with git behind the `Vcs` seam). Every executable is a thin shell
over it, so they share one implementation of the model, staleness, and clean-tree
discipline:

* `llaundry` — the CLI (below).
* `llaundry-tui` — an interactive terminal UI.
* `llaundry-mcp` — a Model Context Protocol server (§5.2).
* `llaundry-work` — the driver that runs an LLM session against a node (§5.3).

### 5.1 The CLI

The `llaundry` binary. The store path defaults to `.llaundry/` and can be
overridden with `--store <dir>` or the `LLAUNDRY_DIR` environment variable.

| Command | Purpose |
|---|---|
| `init` | Create an empty store. |
| `add` | Create a new node; prints its logical id. `-i/--input <file>` declares a pinned input. |
| `link <from> <to>` | Add a typed edge (a new version of `<from>`). |
| `edit <id>` | Produce a new version of a node. |
| `complete <id> -o <file>...` | Commit the produced files with git; store that commit on the node; mark it `done`. `-c/--context <file>` pins used context. |
| `set-status <id> <status>` | Append a status event; `<status>` is one of `open\|in_progress\|done\|failed` (alias: `status`). |
| `show <id>` | Show current version, edges, inputs, context, outputs, and any staleness reasons. |
| `list` | List every node with its current status. |
| `log <id>` | Walk a node's version history (newest first). |
| `stale` | Report nodes that are stale, with explicit reasons. |
| `ready` | List unfinished nodes whose dependencies are all satisfied (done, not stale). |
| `blocked` | List nodes blocked by an unsatisfied dependency, with reasons. |
| `outputs <id>` | Print the output commit a node produced, if any. |
| `origin <commit>` | Find which node produced a given output commit (the inverse of `outputs`). |

### Examples

```sh
llaundry init

# A feature request, then two tasks derived from it, one depending on the other.
REQ=$(llaundry add --type task --title "Add config loading" \
        --body "User wants TOML config support." | awk '{print $1}')

T1=$(llaundry add --type task --title "Define config schema" \
        --derived-from "$REQ" | awk '{print $1}')

T2=$(llaundry add --type task --title "Parse config file" \
        --derived-from "$REQ" --depends-on "$T1" | awk '{print $1}')

llaundry list
llaundry show "$T2"

# Mark the first task done; then revise it — which makes T2 stale.
llaundry set-status "$T1" done
llaundry edit "$T1" --title "Define and validate config schema"
llaundry stale          # -> T2's depends_on edge is now stale

# Implement T2: write the file, then complete it. `complete` git-commits the file
# and stores that commit on the node. Editing the file later makes T2 stale, and
# the reason is a real git diff.
echo 'fn parse() {}' > src/config.rs
llaundry complete "$T2" -o src/config.rs
echo "// hand-edit" >> src/config.rs
llaundry stale          # -> T2: output changed since <commit>: M  src/config.rs
git checkout -- src/config.rs   # restore a clean tree before the next operation

# Declared inputs and recorded context. A node that consumes config.rs declares it
# as an input; completing also records files the agent actually read.
U=$(llaundry add --type task --title "use config" --input src/config.rs | awk '{print $1}')
llaundry complete "$U" -o src/use.rs --context src/helper.rs
# Later, editing src/config.rs (a declared input) or src/helper.rs (recorded
# context) flags U directly — no need to re-version the producer:
#   U: input src/config.rs: content changed (pinned …, now …)
#   U: context src/helper.rs: content changed (pinned …, now …)

# Readiness is derived from dependency status, not stored.
llaundry blocked        # -> lists nodes waiting on a not-yet-done dependency
llaundry set-status "$T1" done
llaundry ready          # -> T2 now appears: its dependency is satisfied

# Provenance, both directions. `outputs` reads the commit off the node; `origin`
# inverts it by scanning. (The output commit is the one `complete` made — it is the
# parent of the store commit, not HEAD; `outputs` gives you the right hash to trace.)
C=$(llaundry outputs "$T2")
llaundry origin "$C"    # -> T2, and the version that produced it
```

### What each command does to the store

Every mutating command below first requires a clean working tree (§2.11) and, after
mutating the store, commits that store change — so the tree is clean between
operations and each is its own git commit.

* **add** — writes one object, creates one ref, appends an `open` status event.
  `--depends-on` / `--derived-from` add edges pinned to the targets' current
  versions; `--input` pins declared input files by their current content.
* **link / edit / complete** — these are *edits*: they read the current version,
  change it, write a **new** object, and move the ref. The previous version stays
  on disk forever (it is the history). `complete` additionally git-commits the
  named output files (the output commit), pins any `--context` files by content,
  stores both on the node, and appends a `done` status event. (`complete` permits
  only the declared outputs to be dirty.)
* **set-status** — appends one immutable event.
* **show / list / log / stale / ready / blocked / outputs / origin** — read-only;
  they rebuild what they need by scanning, holding no persisted index. `stale`
  checks edge pins (against target refs), input/context pins (file content via
  `git hash-object`), outputs (via `git diff` against each node's output commit),
  and whether a `done` status still matches the node's current version (§2.12).
  `ready`/`blocked` derive dependency satisfaction from edges + target statuses
  (§2.10), counting a dependency as done only if its completion covers its current
  version. `outputs` reads the node's stored output commit; `origin` is its inverse,
  derived by scanning every node's version history for the matching `output_commit`
  rather than persisting a second commit→node index — the same store-vs-derive
  choice as the missing index (§2.3). Because `edit` carries the output commit
  forward onto new versions, `origin` returns the *completing* version (the oldest
  bearing that commit), which is unique per `complete`.

### 5.2 The MCP server

The `llaundry-mcp` binary exposes the same operations to an LLM agent over the
[Model Context Protocol](https://modelcontextprotocol.io). It speaks JSON-RPC 2.0
over MCP's stdio transport — one JSON message per line, replies on stdout, logs on
stderr — and, like the rest of the project, is synchronous and dependency-light: the
loop reads a line, dispatches to `llaundry::ops`, writes a line. It implements
`initialize`, `tools/list`, and `tools/call` (plus `ping`); notifications such as
`notifications/initialized` get no reply.

Each tool is its own type implementing a small `Tool` trait — `name`, `description`,
`input_schema`, and `call` — and a `registry()` lists them. `tools/list` maps over
the registry and `tools/call` finds a tool by name, so adding a tool is adding a type
and one registry entry, with no central dispatch match to keep in sync. Every `call`
is a thin wrapper over one `ops::*` function, so an agent gets the same surface as
the CLI. The store path comes from `--store`/`LLAUNDRY_DIR`, and each call opens the
store fresh (via a `Ctx`) — so `initialize`/`tools/list` work before a store exists
and an agent can create one with `init_store`.

| Tool | Wraps |
|---|---|
| `init_store` | `Store::init` |
| `add_node`, `link_nodes`, `edit_node`, `complete_node`, `set_status` | the mutating `ops::*` |
| `show_node`, `list_nodes`, `node_history` | read-only reads |
| `stale_nodes`, `ready_nodes`, `blocked_nodes` | the derived queries |
| `node_origin`, `node_outputs` | provenance (§2.8) |

The mutating tools inherit the clean-tree rule (§2.11): they require a git
repository and commit their own store change, and surface any refusal as an MCP
tool error (`isError: true`) rather than a protocol failure. Sandboxing an agent to
its declared inputs (§2.9) remains a runtime concern the server does not yet
enforce.

### 5.3 The worker

The `llaundry-work` binary is the driver for actually *doing* a node's work with an
LLM. It launches a **session** against one node: it loads the node, refuses to start
if the node is blocked (its `depends_on` targets aren't satisfied — override with
`--force`), builds a prompt from the node's type, title, body, edges, and inputs,
and hands that to a backend. Once the prompt is built, the session carries no store
handle — a backend needs nothing more from the database.

The LLM is behind a `Backend` trait (`run` a session; `describe` it for
`--dry-run`), so engines are swappable without touching the launcher. The first
backend, `ClaudeCode`, shells out to `claude -p` deliberately sandboxed:

* `--mcp-config <json>` + `--strict-mcp-config` — expose **only** the `llaundry`
  MCP server (§5.2), ignoring any user/project MCP config.
* `--allowedTools mcp__llaundry` — grant every tool of that server and nothing
  else. In non-interactive `-p` mode any tool not listed is denied, so no built-in
  tools (shell, file, network) are reachable, and permissions are not bypassed.

So the model can act on the graph and nothing else. Because it has no file tools, a
session produces no output files, so the prompt steers completion toward
`set_status … done` rather than `complete_node` (which commits produced files). A
future backend with file access would use `complete_node` to capture real outputs.

Command construction is separated from execution (`ClaudeCode::command`), so the
exact sandboxed invocation is unit-tested and shown verbatim by `--dry-run` without
running or even installing Claude Code.

---

## 6. Deliberately out of scope (for now)

* **Context *enforcement*.** Inputs and used context are *recorded* and pinned
  (§2.9), but nothing yet *prevents* an agent from reading undeclared files — that
  sandboxing is a runtime concern the MCP server (§5.2) does not yet impose.
* **Reverse-edge queries** ("what depends on X") beyond the `stale` scan, and any
  persisted index — would be an in-memory cache in a long-running process.
* **Executing builds and verifications.** Nodes can be *typed* `build` /
  `verification`, but running them is not implemented.
* **Transitive staleness.** Each node's own edges, inputs, context, and outputs are
  checked; a dependent is not auto-flagged because something upstream of *its*
  target moved. (Content-pinned inputs reduce the need: a consumer that pins an
  input is flagged directly when that content changes — see §2.9.)
* **Output commit policy.** `complete` makes a partial commit of exactly the named
  files plus a separate commit for the store. It does not squash, sign, or let you
  reuse an existing commit; those are easy future options.
* **Object sharding** (`objects/ab/cdef...`) and an `fsck`/verify command that
  re-derives and checks every hash.
* **Adopting git's own object ids as the *node* hash** (§2.4). Outputs already
  delegate to git (§2.8); node identity and edge pins still use our own `sha256`,
  so those parts of the tool work without git. Unifying them is a separate call.
