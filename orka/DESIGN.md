# Orka design

## Purpose

Orka orchestrates graph-driven agent work. It uses Linka to discover and track
work and Driva to execute agent commands in isolation. Orka owns coordination and
durable attempts; it does not implement Docker execution or human review.

## Boundaries

Orka depends on two narrow capabilities:

```rust
trait WorkGraph {
    // Read ready work and pinned context; submit a version-checked outcome.
}

trait IsolatedExecutor {
    // Run a command with a concrete filesystem and network capability grant.
}
```

Production adapters use Linka and Driva. Tests can use fakes. Orchestration
logic must not reach into either application's on-disk representation.

## Attempt lifecycle

1. Select eligible work from Linka according to orchestration policy.
2. Freeze the node definition, dependency results, explicit context, and
   project input version in a durable attempt.
3. Choose the exact context mounts, network policy, agent command, and prior
   context needed for the attempt, then construct a Driva execution request.
4. Durably record that request before starting the command.
5. Capture transcript, exit evidence, declared outputs, and agent outcome.
6. Re-check the frozen Linka versions. Never silently complete stale work.
7. Submit a version-checked result to Linka or retain a recoverable failed or
   interrupted attempt.

Retries and resumed agent conversations remain linked to their attempt. Each
one is a new Driva execution; any prior agent context is prepared by Orka and
passed through the command or explicitly mounted files. A later execution does
not broaden mounts, network access, or graph authority without a new policy
decision.

## Agent authority

Orka turns graph context into a concrete capability grant. An agent sees only
the nodes, files, tools, mounts, and network access needed for its attempt.
Authorization is enforced by adapters and scoped tools, not merely described
in a prompt. Backend/model evidence comes from the harness rather than from
agent claims.

## Durability and recovery

An attempt is written before external side effects. It records frozen inputs,
the Driva request, transcript references, exit evidence, and final submission
state. Orka records enough around the synchronous execution to recover its own
attempt and makes completion idempotent.

## Non-goals

- Implementing isolation mechanics or process stdio (Driva).
- Owning node-graph semantics or storage (Linka).
- Review comments, suggested edits, approval, or publication (Nota).
