# Orka

Orka orchestrates isolated agent attempts for work in a Linka store: it uses
Linka's public API to discover, freeze, and record work, and Driva to run agent
commands in isolation. Orka orchestrates Linka specifically — it has no generic
graph backend. See [`DESIGN.md`](DESIGN.md) for the ownership boundary and
attempt lifecycle. [`SIMPLIFICATION_TASKS.md`](SIMPLIFICATION_TASKS.md) is the
plan this code follows; [`TASKS.md`](TASKS.md) is superseded history.

## What an attempt does

```text
select Linka-ready node ──► snapshot Linka work input ──► record attempt (.orka/attempts/<id>/)
      ──► prepare worktree (orka/attempts/<id> branch at the frozen revision)
      ──► record request ──► run agent via Driva (bwrap/podman/docker, deny-by-default)
      ──► capture transcript + exit evidence ──► read declared outcome
      ──► version-checked submit + Linka candidate ──► seal ──► clean up
```

The attempt durably stores Linka's exact `WorkSnapshot`. Before submitting a
successful outcome, Orka atomically attaches the attempt input, exact prompt,
execution request, transcript, harness evidence, and raw declared outcome to
the source node through Linka's generic opaque attachment API. The evidence
needed to understand a produced output therefore travels with the Linka Git
repository even if local `.orka/` state is later unavailable. Success or
failure is submitted against the snapshot through Linka's version-checked
`capture_submission`. Every step is recorded before its side effect, so `orka
recover` can classify any crash from the files present and finish the
idempotent remainder. Recovery also backfills complete evidence for outputs
sealed by older Orka versions. Stale work — a graph that moved between snapshot
and submit — is refused and sealed as such, never silently completed.

## Use

Run from a Linka workbench (the directory holding `.linka/` and `project/`).
Create the default configuration beside them with:

```text
orka init
```

The generated `orka.toml` selects Orka's non-interactive Codex profile and an
explicit Driva isolation backend:

```toml
[agent]
kind = "codex"

[isolation]
backend = "bwrap"
rootfs = "/"
tmpfs = ["/root"]
```

Orka owns the Codex command line, workspace trust, credential grant,
environment, and prompt/outcome protocol. It sends the resulting concrete
execution request to Driva, which supplies only request validation and the
Bubblewrap, Podman, or Docker isolation mechanism. The default uses the host's
`codex` executable through a read-only host rootfs with private `/root` and
`/tmp` state.

Literal commands remain available for custom container images:

```toml
[agent]
command = ["sh", "-c", "…runs inside the container…"]

[isolation]
backend = "podman"                       # or "docker"
image = "docker.io/library/busybox:latest"
```

```text
orka ready               list workable nodes
orka init                create a default orka.toml (never overwrite one)
orka run [NODE]          run one attempt (first ready node when omitted)
orka attempts            list recorded attempts
orka show ATTEMPT        one attempt's durable record
orka candidates          list project candidates with their source nodes
orka candidate CANDIDATE show a candidate and its patch
orka accept CANDIDATE    record exact acceptance in Linka
orka reject CANDIDATE    reject it and make its source retryable
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
orka review finish NODE --verdict VERDICT
                         submit review evidence to the verification node
orka review abandon NODE [--notes NOTES]
                         stop a review (also available as `review stop`)
orka recover             classify and finish unfinished attempts
```

The agent command executes inside the isolated environment with the attempt
worktree mounted writable at `/tmp/orka/workspace` and an exchange directory
at `/tmp/orka/exchange` (`$ORKA_PROMPT` in, `$ORKA_OUTCOME` out). These are
Orka's stable internal execution paths; the host worktree remains unique to
the attempt. The agent declares its outcome by writing `outcome.toml`; see
`src/outcome.rs` for the contract.

When a successful attempt produces project files, Orka registers a first-class
Linka candidate and prints its id and follow-up commands. Linka reports the
source node as awaiting integration—not stale and not ready for duplicate
machine work—until that exact candidate is decided and published:

```text
orka candidates
orka candidate CANDIDATE
orka accept CANDIDATE --notes "reviewed"
orka publish CANDIDATE
```

The candidate list connects Linka's candidate id to its source node, branch,
target, and opaque Orka attempt identity. Linka owns the decision and derives
publication from Git history; Orka only supplies an attempt-oriented UI and
patch view. The patch base comes from the attempt input attached durably to the
Linka node, with local `.orka/` state used only as a compatibility fallback.
Acceptance pins the exact artifact and previous target commit.
Publication refuses dirty or concurrently moved targets and is safe to retry
after a crash.

Worktree cleanup retains the `orka/attempts/<attempt-id>` candidate branch for
every sealed attempt, including stale submissions and recorded failures. This
keeps attempted work available for later inspection or recovery. One narrow
case is rolled back completely: when the executor returns no exit evidence and
the workspace and branch still exactly match their frozen input, Orka removes
the empty worktree, candidate branch, and attempt record. Changed work is never
discarded implicitly. `orka recover` applies the same rule to empty interrupted
attempts left by older versions.

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
Nota branch. `orka review finish` records the chosen verdict and Git evidence
as the verification result; it does not implicitly accept, reject, or publish
the candidate. `orka review list` shows unfinished bindings, including starts
interrupted before branch creation. `orka review abandon` (or `review stop`)
records a failed verification with abandonment evidence and preserves the Nota
branch for inspection.
