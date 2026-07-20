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
      ──► record request ──► run agent via Driva (podman/docker, deny-by-default)
      ──► capture transcript + exit evidence ──► read declared outcome
      ──► version-checked submit + Linka candidate ──► seal ──► clean up
```

The attempt durably stores Linka's exact `WorkSnapshot`, and success or failure
is submitted against that snapshot through Linka's version-checked
`capture_submission`. Every step is recorded before its side effect, so `orka
recover` can classify any crash from the files present and finish the idempotent
remainder. Stale work — a graph that moved between snapshot and submit — is
refused and sealed as such, never silently completed.

## Use

Run from a Linka workbench (the directory holding `.linka/` and `project/`).
Create the default configuration beside them with:

```text
orka init
```

The generated `orka.toml` selects Driva's non-interactive Codex template:

```toml
[agent]
template = "codex-exec"
```

Install its prepared runtime once with `driva runtime install codex@latest`.
Orka replaces the template's generic workspace mount with the isolated attempt
worktree, adds its prompt and outcome exchange mount, and preserves the
template's rootfs, credential mounts, environment, and network policy.

Literal commands remain available for custom container images:

```toml
[agent]
command = ["sh", "-c", "…runs inside the container…"]

[isolation]
backend = "podman"                       # default; or "docker"
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
orka recover             classify and finish unfinished attempts
```

The agent command executes inside the isolated environment with the attempt
worktree mounted writable at `/workspace` and an exchange directory
(`$ORKA_PROMPT` in, `$ORKA_OUTCOME` out). It declares its outcome by
writing `outcome.toml`; see `src/outcome.rs` for the contract.

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
patch view. The patch base comes from Orka's durable attempt input rather than
being duplicated in Linka. Acceptance pins the exact artifact and previous target commit.
Publication refuses dirty or concurrently moved targets and is safe to retry
after a crash.

Worktree cleanup retains the `orka/attempts/<attempt-id>` candidate branch for
every sealed attempt, including stale submissions and recorded failures. This
keeps attempted work available for later inspection or recovery. Orka does not
currently prune attempt records or their candidate branches implicitly.

## Source

Source from the former combined execution/review application
(`verka-work`) is parked in
[`../verka-work-reference/`](../verka-work-reference/) as reference
material only.
