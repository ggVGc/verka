# llaundry — database design

This document describes the local database that backs llaundry, the reasoning
behind it, and the small CLI tool that operates on it. It is a record of the
design discussion as much as a specification.

llaundry's premise (see `README.md` / `ideas.wiki`): **the prompt history is the
story, not the output code.** Work is driven by a graph of nodes — descriptions,
tasks, implementations, builds, verifications — each carrying the prompt and
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
* **Minimal.** The first cut is just the database plus a tiny CLI. No MCP, no
  server, no build/verification execution.

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
create a **new node** (e.g. a description "search the web for X") that *produces an
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
to = "desc-01J8XQ2A..."
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
valid TOML while adding only new lines. The current status is the last event.
(`at` is Unix milliseconds — deliberately dependency-free; a future version may
switch to RFC 3339.)

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

The `complete` command goes one step further and *drives* git: it commits the
produced files and stores the commit hash on the node (§2.8). So `complete`
requires a git repository (with a configured identity); the other commands do not.

To keep that git dependency from leaking into tests, all git interaction goes
through a small `Vcs` trait (`capture`, `commit_store`, `drift`, `content_id`). The
real implementation (`GitVcs`) shells out to `git`; `complete`, `add` (for pinning
inputs), and the staleness check take `&dyn Vcs`. Unit tests inject an in-memory
`FakeVcs`, so the store, hashing, edge/input/context/output staleness, and the
`complete` flow are all exercised with **no git binary, no repository, and no
configured identity** — fast, deterministic, self-standing. A separate (optional)
integration test can exercise real `GitVcs`.

---

## 5. The CLI

A single binary, `llaundry`. The store path defaults to `.llaundry/` and can be
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

### Examples

```sh
llaundry init

# A feature request, then two tasks derived from it, one depending on the other.
REQ=$(llaundry add --type description --title "Add config loading" \
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
```

### What each command does to the store

* **add** — writes one object, creates one ref, appends an `open` status event.
  `--depends-on` / `--derived-from` add edges pinned to the targets' current
  versions; `--input` pins declared input files by their current content.
* **link / edit / complete** — these are *edits*: they read the current version,
  change it, write a **new** object, and move the ref. The previous version stays
  on disk forever (it is the history). `complete` additionally git-commits the
  named output files (the output commit), pins any `--context` files by content,
  stores both on the node, appends a `done` status event, and commits the store
  change.
* **set-status** — appends one immutable event; touches no object and no ref.
* **show / list / log / stale / ready / blocked** — read-only; they rebuild what
  they need by scanning, holding no persisted index. `stale` checks edge pins
  (against target refs), input/context pins (file content via `git hash-object`),
  and outputs (via `git diff` against each node's output commit). `ready`/`blocked`
  derive dependency satisfaction from edges + target statuses (§2.10).

---

## 6. Deliberately out of scope (for now)

* **MCP / server.** This is just the database and a CLI.
* **Context *enforcement*.** Inputs and used context are *recorded* and pinned
  (§2.9), but nothing yet *prevents* an agent from reading undeclared files — that
  sandboxing is a runtime/MCP concern.
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
