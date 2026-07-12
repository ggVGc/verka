# Orka

Orka is the orchestrator: it uses Linka to discover and track work and Driva
to run agent commands in isolation. See [`DESIGN.md`](DESIGN.md) for the
accepted boundary and attempt lifecycle, and [`TASKS.md`](TASKS.md) for the
implementation plan this code follows.

## What an attempt does

```text
select ready node ──► freeze graph input ──► record attempt (.orka/attempts/<id>/)
      ──► prepare worktree (orka/attempts/<id> branch at the frozen commit)
      ──► record request ──► run agent via Driva (podman/docker, deny-by-default)
      ──► capture transcript + exit evidence ──► read declared outcome
      ──► version-checked submit to Linka ──► seal ──► clean up (never dirty trees)
```

Every step is durably recorded before its side effect, so `orka recover`
can classify any crash from the files present and finish the idempotent
remainder. Stale work — a graph that moved between freeze and submit — is
refused and sealed as such, never silently completed.

## Use

Run from a Linka workbench (the directory holding `.linka/` and `project/`)
with an `orka.toml` beside them:

```toml
[agent]
command = ["sh", "-c", "…runs inside the container…"]

[isolation]
backend = "podman"                       # default; or "docker"
image = "docker.io/library/busybox:latest"
```

```text
orka ready               list workable nodes
orka run [NODE]          run one attempt (first ready node when omitted)
orka attempts            list recorded attempts
orka show ATTEMPT        one attempt's durable record
orka recover             classify and finish unfinished attempts
```

The agent command executes inside the isolated environment with the attempt
worktree mounted writable at `/workspace` and an exchange directory at
`/orka` (`$ORKA_PROMPT` in, `$ORKA_OUTCOME` out). It declares its outcome by
writing `outcome.toml`; see `src/outcome.rs` for the contract. Publication of
an accepted candidate branch into the project checkout is review's decision,
not Orka's.

## Source

Source from the former combined execution/review application
(`llaundry-work`) is parked in
[`../llaundry-work-reference/`](../llaundry-work-reference/) as reference
material only.
