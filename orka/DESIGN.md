# Orka design

## Purpose

Orka orchestrates graph-driven agent work. It uses Linka to discover and track
work and Driva to execute isolated agent sessions. Orka owns coordination and
durable attempts; it does not implement Docker execution or human review.

## Boundaries

Orka depends on two narrow capabilities:

```rust
trait WorkGraph {
    // Read ready work and pinned context; submit a version-checked outcome.
}

trait SessionRunner {
    // Start, attach to, continue, wait for, and terminate an agent session.
}
```

Production adapters use Linka and Driva. Tests can use fakes. Orchestration
logic must not reach into either application's on-disk representation.

## Attempt lifecycle

1. Select eligible work from Linka according to orchestration policy.
2. Freeze the node definition, dependency results, explicit context, and
   project input version in a durable attempt.
3. Choose the exact context mounts, network policy, agent command, and prior
   context needed for a Driva session.
4. Start Driva and durably record the returned session identity before relying
   on its output.
5. Capture transcript, exit evidence, declared outputs, and agent outcome.
6. Re-check the frozen Linka versions. Never silently complete stale work.
7. Submit a version-checked result to Linka or retain a recoverable failed or
   interrupted attempt.

Retries and resumed sessions remain linked to their attempt. A retry policy
may create a new Driva session; continuation may attach to an existing waiting
session. Neither operation broadens mounts, network access, or graph authority
without an explicit new policy decision.

## Agent authority

Orka turns graph context into a concrete capability grant. An agent sees only
the nodes, files, tools, mounts, and network access needed for its attempt.
Authorization is enforced by adapters and scoped tools, not merely described
in a prompt. Backend/model evidence comes from the harness rather than from
agent claims.

## Durability and recovery

An attempt is written before external side effects. It records frozen inputs,
the Driva request and session identity, lifecycle transitions, transcript
references, exit evidence, and final submission state. Recovery reconciles
that record with Driva and Linka and makes completion idempotent.

## Non-goals

- Implementing container lifecycle or agent stdio (Driva).
- Owning node-graph semantics or storage (Linka).
- Review comments, suggested edits, approval, or publication (Nota).
