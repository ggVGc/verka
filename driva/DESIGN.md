# Driva design

## Purpose

Driva is a small standalone runner for one agent session in a Docker
container. Callers explicitly choose the agent command, context mounts, and
whether networking is available. Driva can be used without Linka, Orka, Nota,
or any repository-specific layout.

## Session request

A request contains:

- container image and agent command;
- an explicit list of read-only or read-write mounts;
- environment values and secrets supplied through deliberate channels;
- network mode, disabled by default;
- optional prior agent context;
- stdio attachment mode and an optional wait-for-continuation policy;
- resource limits and termination timeout.

There are no implicit host-directory mounts. Driva validates and reports the
effective request before starting Docker. Network access is opt-in and its
effective mode is included in session evidence.

## Lifecycle

```text
created -> running -> waiting -> running -> finished -> removed
                     |                     |
                     +------ terminate ----+
```

`waiting` means the agent process and container remain available for later
stdio continuation. A caller can attach, provide more input, detach while
leaving it waiting, or explicitly finish it. Prior context may instead seed a
new session when the agent backend supports that form of continuation.

Driva owns the container it creates. When a session finishes, is cancelled,
times out, or encounters a runner failure, Driva stops and removes that
container. Cleanup is idempotent and is attempted during recovery after a
Driva restart. A deliberate waiting session is the only normal state in which
the container survives a detached client.

## I/O and evidence

Stdin, stdout, and stderr are transported without interpreting agent content.
The runner applies backpressure and preserves ordering information needed by a
caller to retain a transcript. Results include the session ID, Docker
container ID, effective isolation settings, timestamps, exit status, cleanup
status, and runner errors.

Driva may retain minimal continuation metadata when configured. It does not
invent a task, attempt, review, or graph schema.

## Security

- Mounts are allowlisted per request and canonicalized before launch.
- Network connectivity is disabled unless explicitly requested.
- Secrets are not copied into transcripts or persisted request dumps.
- Containers receive bounded resources and a termination grace period.
- A continuation token identifies a session but does not authorize broader
  mounts or network access.

## Interfaces

The library/service contract supports start, attach, send input, detach and
wait, inspect, finish, and terminate. The CLI exposes the same lifecycle for
shell use. Docker is behind a process/container driver so lifecycle behavior
can be tested without a daemon.

## Non-goals

- Selecting work or retrying tasks.
- Understanding Linka nodes or Orka attempts.
- Parsing agent responses into work results.
- Reviewing or publishing changes.
