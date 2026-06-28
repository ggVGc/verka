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

[[edges]]
to = "desc-01J8XQ2A..."
rel = "derived_from"
pin = "4a7e..."

[[edges]]
to = "task-01J8XQ4P..."
rel = "depends_on"
pin = "1b2c..."
```

`body.md` is free-form Markdown.

**Hash.** `hash = sha256(meta.toml bytes || 0x00 || body.md bytes)`, hex-encoded.
It is computed over the exact bytes written, and is never stored inside the record.
A given (meta, body) pair therefore always lands at the same path — writes are
idempotent, and identical content is deduplicated automatically.

**Identity includes everything definitional**: `logical_id`, `type`, `title`,
`parent`, `edges`, and the body. Change any of them and you get a new hash, i.e. a
new version. `parent` links versions into a history chain.

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

---

## 5. The CLI

A single binary, `llaundry`. The store path defaults to `.llaundry/` and can be
overridden with `--store <dir>` or the `LLAUNDRY_DIR` environment variable.

| Command | Purpose |
|---|---|
| `init` | Create an empty store. |
| `add` | Create a new node; prints its logical id. |
| `link <from> <to>` | Add a typed edge (a new version of `<from>`). |
| `edit <id>` | Produce a new version of a node. |
| `set-status <id> <status>` | Append a status event (alias: `status`). |
| `show <id>` | Show current version, edges (with staleness), and status. |
| `list` | List every node with its current status. |
| `log <id>` | Walk a node's version history (newest first). |
| `stale` | Report nodes whose edges point at outdated target versions. |

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
```

### What each command does to the store

* **add** — writes one object, creates one ref, appends an `open` status event.
  `--depends-on` / `--derived-from` add edges pinned to the targets' current
  versions.
* **link / edit** — these are *edits*: they read the current version, change it,
  write a **new** object, and move the ref. The previous version stays on disk
  forever (it is the history).
* **set-status** — appends one immutable event; touches no object and no ref.
* **show / list / log / stale** — read-only; they rebuild what they need by
  scanning, holding no persisted index.

---

## 6. Deliberately out of scope (for now)

* **MCP / server / context enforcement.** This is just the database and a CLI.
* **Reverse-edge queries** ("what depends on X") beyond the `stale` scan, and any
  persisted index — would be an in-memory cache in a long-running process.
* **Executing builds and verifications.** Nodes can be *typed* `build` /
  `verification`, but running them is not implemented.
* **Object sharding** (`objects/ab/cdef...`) and an `fsck`/verify command that
  re-derives and checks every hash.
* **Adopting git's own object ids as the node hash** (§2.4), and `sha256`
  object-format repos. The current scheme keeps the tool self-contained.
