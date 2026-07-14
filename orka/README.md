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
      ──► version-checked submit to Linka ──► seal ──► clean up (never dirty trees)
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
orka recover             classify and finish unfinished attempts
```

The agent command executes inside the isolated environment with the attempt
worktree mounted writable at `/workspace` and an exchange directory
(`$ORKA_PROMPT` in, `$ORKA_OUTCOME` out). It declares its outcome by
writing `outcome.toml`; see `src/outcome.rs` for the contract. Publication of
an accepted candidate branch into the project checkout is review's decision,
not Orka's.

Worktree cleanup retains the `orka/attempts/<attempt-id>` candidate branch for
every sealed attempt, including stale submissions and recorded failures. This
keeps attempted work available for later inspection or recovery. Orka does not
currently prune attempt records or their candidate branches implicitly.

## Source

Source from the former combined execution/review application
(`llaundry-work`) is parked in
[`../llaundry-work-reference/`](../llaundry-work-reference/) as reference
material only.
