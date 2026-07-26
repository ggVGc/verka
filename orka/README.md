# Orka

Orka orchestrates isolated agent attempts for work in a Linka store: it uses
Linka's public API to discover, freeze, and record work, and Driva to run agent
commands in isolation. Orka orchestrates Linka specifically — it has no generic
graph backend. See [`DESIGN.md`](DESIGN.md) for the ownership boundary and
attempt lifecycle.

## What an attempt does

```text
select Linka-ready node ──► snapshot Linka work input ──► record attempt (.orka/attempts/<id>/)
      ──► prepare audited worktree (orka/attempts/<id> branch at the frozen revision)
      ──► record request ──► run agent via Driva (bwrap, deny-by-default)
      ──► capture agent events + file reads + diagnostics + exit evidence
      ──► read declared outcome ──► version-checked submit + observed context pins
      ──► Linka candidate ──► seal ──► clean up
```

Until registration, the attempt stores Linka's exact `WorkSnapshot` as local
recovery state. When Linka accepts success or failure, Orka records the result
and its complete evidence attachment batch in the same Linka store commit. The
evidence needed to understand finished work therefore travels with the Linka
Git repository. After safe workspace cleanup, Orka removes the registered
attempt directory; `.orka/` is ignored, ephemeral coordination state rather
than an archive. Every pre-registration step is recorded before its side
effect, so `orka recover` can classify a crash and finish the idempotent
remainder. Stale work — a graph that moved between snapshot and submit — is
refused and retains local recovery state, never silently completed.

## Use

Run from a Linka workbench (the directory holding `.linka/` and `project/`).
Create the default configuration beside them with:

```text
orka init
```

The generated `orka.toml` selects Genta's non-interactive Codex profile and an
explicit Driva isolation backend:

```toml
[agent]
kind = "codex"

[isolation]
backend = "bwrap"
rootfs = "/"
tmpfs = ["/root"]
```

Genta supplies the Codex command line, mounts, environment, and output
protocol; Orka owns which profile to select, workspace trust, the credential
grant, and the prompt/outcome protocol. It sends the resulting concrete
execution request to Driva, which supplies only request validation and the
Bubblewrap isolation mechanism. The default uses the host's `codex` executable
through a read-only host rootfs with private `/root` and `/tmp` state.

Literal commands remain available for custom sandboxes:

```toml
[agent]
command = ["sh", "-c", "…runs inside the sandbox…"]

[isolation]
backend = "bwrap"
rootfs = "/"
```

```text
orka ready               list workable nodes
orka init                create a default orka.toml (never overwrite one)
orka run [NODE]          run one attempt (first ready node when omitted)
  --auto-accept          on success, accept through a review node and publish
orka attempts            list recorded attempts
orka show ATTEMPT        one attempt's durable record
orka candidates          list project candidates with their source nodes
orka candidate CANDIDATE show a candidate and its patch
orka accept CANDIDATE VERIFICATION
                         recover an accepted review decision
orka reject CANDIDATE VERIFICATION
                         recover a rejected review decision
orka publish CANDIDATE   recoverably fast-forward the recorded target
orka audit               verify evidence for every Orka-produced output
orka review list         list active reviews
orka review start CANDIDATE [--enter]
                         create a review and optionally prepare its managed tree
orka review resume NODE  finish an interrupted review-branch creation
orka review enter NODE   create or reuse its managed worktree and print its path
orka review worktree NODE [--print-path]
                         create or reuse its managed worktree
orka review worktrees    inspect managed review worktrees
orka review cleanup NODE remove its managed worktree when clean
orka review show NODE    show the binding and Git-native review entries
orka review finish NODE --outcome accepted|rejected
                         submit and apply the review outcome
orka review abandon NODE [--notes NOTES]
                         stop a review (also available as `review stop`)
orka recover             classify and finish unfinished attempts
```

The agent command executes inside the isolated environment with the attempt
file tree mounted writable at `/tmp/orka/workspace`, private per-attempt Git
metadata, and an exchange directory at `/tmp/orka/exchange` (`$ORKA_PROMPT`
in, `$ORKA_OUTCOME` out). The project repository's shared `.git` is never
writable in the sandbox. Before launch, Orka runs its own Git writability probe
through the exact sandbox grant. After execution, Orka validates repository
identity, ancestry, and connectivity before interpreting `outcome.toml`.
Failure of either gate records a workspace-integrity failure and cannot create
a Linka result, candidate, evidence attachment, or project commit.

The Codex profile runs `codex exec --json`. Orka keeps the provider's exact
stdout in `events.raw.jsonl`, projects it into stable Orka events in
`events.v1.jsonl`, derives a readable `transcript.log`, and keeps stderr in
`diagnostics.log`. On Linux, it also watches the attempt file tree and records
project file reads in `accesses.v1.jsonl`; accepted results pin those files by
their content at the frozen input revision. During `orka run`, the CLI follows
the raw journal and renders commands, tools, changed files, Markdown agent
messages, failures, and token usage as they arrive. Terminal control sequences
in agent-produced text are removed before rendering. Literal `[agent].command` profiles retain plain
stdout in `transcript.log` and stderr in `diagnostics.log`.

Every completed `file_change` event also creates a checkpoint commit through a
temporary Git index and records it in `file-changes.v1.jsonl`. The commits form a
chain retained at `refs/orka/file-changes/<attempt-id>` without moving the
agent repository's HEAD or index. This preserves each reported intermediate file
state without changing Linka's final declared-output capture.

Attempt and workspace records are strict, versioned formats. Orka supports only
the current audited-worktree layout: unsupported schemas and records without
the shared-repository audit are refused rather than migrated.

Human-facing views are projected through Orka's provider-independent
`WorkLogBlock` format. Markdown prose and fenced code are separate content
blocks, including the fence's language, so the terminal and `orka-web` share
the same interpretation while choosing presentation appropriate to each UI.

Driva remains unaware of this protocol: it only validates the capability grant
and transports separate stdin, stdout, and stderr streams. Provider decoding,
durable agent transcripts, and presentation remain Orka responsibilities.

When a successful attempt produces project files, Orka registers a first-class
Linka candidate and prints its id and follow-up commands. Linka reports the
source node as awaiting integration—not stale and not ready for duplicate
machine work—until that exact candidate is decided and published:

```text
orka candidates
orka candidate CANDIDATE
orka review start CANDIDATE
orka review finish VERIFICATION --outcome accepted
orka publish CANDIDATE
```

For trusted automated workflows, `orka run [NODE] --auto-accept` creates a
machine-assigned verification node after a successful attempt, records its
accepted result, and publishes the candidate. It does nothing after failed or
invalid work. If publication cannot fast-forward (for example because the
checkout is dirty), the accepted candidate is retained and can be retried with
`orka publish CANDIDATE`.

The candidate list connects Linka's candidate id to its source node, branch,
target, and opaque Orka attempt identity. Linka validates the exact verification
authorizing the decision and derives publication from Git history; Orka supplies
the attempt-oriented UI and coordinates Nota evidence with that verification.
patch view. The patch base comes from the attempt input attached durably to the
Linka node.
Acceptance pins the exact artifact and previous target commit.
Publication refuses dirty or concurrently moved targets and is safe to retry
after a crash.

Agents work in ordinary linked worktrees on
`orka/attempts/<attempt-id>` branches. Orka audits the shared Git repository
before and after execution: protected refs, configuration, hooks, alternates,
worktree registrations, object format, connectivity, ancestry, and worktree
cleanliness must remain valid. Only attempt-owned refs may change. An
unexpected shared-Git mutation fails the attempt and retains its worktree.
Cleanup removes a successfully promoted linked worktree while preserving its
candidate branch, then removes the local attempt record once Linka holds the
accepted result and evidence. One narrow pre-registration case is also rolled
back completely: when the executor returns no exit evidence and the worktree
still exactly matches its frozen input, Orka removes the empty worktree, branch,
and attempt record.

## Candidate reviews

Orka can bind an exact Linka candidate to a Git-native Nota review. `orka
review start` creates and snapshots a Linka verification node, records the
binding under `.orka/reviews/`, and starts a Nota branch at the candidate's
artifact commit. Starting it again while the review is active resumes that
binding instead of creating another verification. Add `--enter` to create the
canonical worktree at `.orka/review-worktrees/<verification>/` and print its
path. `orka review enter NODE` reuses it later and prints only the directory,
so a caller may run `cd "$(orka review enter NODE)"`. Reviewers use `nota note`
for prose comments and ordinary `git add` and `git commit` commands for
suggested edits inside that tree. `orka review worktree NODE --print-path`
offers the same path-only output for editor integrations.

`orka review worktrees` reports clean and dirty managed trees. `orka review
cleanup` removes only a clean, correctly registered tree and preserves the
Nota branch. `orka review finish` submits an `accepted` or `rejected` result
with its Git evidence; Linka atomically records that exact verification as the
candidate decision. Publication remains explicit. `orka review list` shows unfinished
bindings, including starts interrupted before branch creation. `orka review
abandon` (or `review stop`) records an `abandoned` verification outcome without
deciding the candidate and preserves the Nota branch for inspection.
