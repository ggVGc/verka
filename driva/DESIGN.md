# Driva design

## Purpose

Driva is a small standalone application for running a command in an isolated
environment. It provides a convenient, reusable interface for both manual use
and programmatic callers such as Orka.

Driva does not implement isolation itself. Its core validates a portable
execution request and delegates it to an isolation backend. Bubblewrap is the
default backend for lightweight synchronous Linux execution; Podman and Docker
are also supported. Backend-specific concepts are not part of the core
interface.

The distinguishing policy is deny by default:

- host files are unavailable unless explicitly mounted;
- mounts are read-only unless explicitly writable;
- command-line PATH additions are mounted read-only and prepended to the
  isolated executable search path;
- networking is disabled unless explicitly enabled; and
- the isolated environment is removed when the command finishes.

Driva's execution core is a general command runner with no knowledge of code
agents, Linka nodes, Orka attempts, prompts, transcripts, or reviews. The CLI
may provide named policy-and-command templates for common tools, but they
compile to the same backend-independent `ExecutionRequest`.

Built-in templates are TOML files under `templates/`. They are embedded in the
binary for reliable distribution and deserialized through the same
`TemplateConfig` schema as project-defined templates.

## User interface

The initial CLI has two operations:

```text
driva run [OPTIONS] -- COMMAND [ARG...]
driva shell [OPTIONS]
```

For example:

```sh
driva run --write . -- cargo test
driva run --read ~/.cargo/registry --write . --network -- cargo update
driva shell --write .
```

`run` executes one command, connects it to the caller's standard streams, and
returns its exit status. `shell` is the same operation with the configured
interactive shell as its command.

`--dry-run` reports the effective policy and backend invocation without
starting an environment. This makes configuration and one-off overrides
inspectable before execution.

## Configuration

A project may contain a `driva.toml` with reusable defaults. Command-line
options override them for one invocation. An initial configuration could look
like:

```toml
[isolation]
backend = "bwrap"

[isolation.bwrap]
rootfs = "/var/lib/driva/rootfs/rust"
workdir = "/workspace"

[[mount]]
source = "."
destination = "/workspace"
access = "write"

[[mount]]
source = "~/.cargo/registry"
destination = "/cargo/registry"
access = "read"

[network]
enabled = false
```

Backend-specific settings remain outside the portable execution request, but
configuration, templates, and command-line overrides resolve through one
launch layer before the selected adapter is constructed. Scalar precedence is
CLI, then template, then project configuration.

## Core interface

The CLI and programmatic callers use the same library operation:

```rust
pub trait Isolation {
    fn run(
        &self,
        request: &ExecutionRequest,
        io: ExecutionIo,
    ) -> Result<ExecutionOutcome>;
}
```

The portable request contains only behavior Driva intends to support across
backends:

```rust
pub struct ExecutionRequest {
    pub command: Vec<OsString>,
    pub working_directory: PathBuf,
    pub mounts: Vec<Mount>,
    pub environment: BTreeMap<OsString, OsString>,
    pub network: bool,
    pub interactive: bool,
}

pub struct Mount {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub access: MountAccess,
}

pub enum MountAccess {
    ReadOnly,
    ReadWrite,
}

pub struct ExecutionOutcome {
    pub exit: ProcessExit,
    pub evidence: ExecutionEvidence,
}

pub struct ExecutionEvidence {
    pub isolation_backend: String,
    pub effective_policy: EffectivePolicy,
    pub started_at: SystemTime,
    pub finished_at: SystemTime,
}
```

Commands are represented as a program and arguments, not as a shell string.
Driva transports stdin, stdout, and stderr without interpreting their content.
The backend translates the request into its native invocation, forwards
signals where possible, waits for the command, and cleans up the environment.

The exact Rust types may change during implementation; the important boundary
is that the trait describes an isolated process rather than exposing Docker
operations such as creating or removing containers.

## Validation and policy

Before invoking a backend, Driva:

- resolves and validates every host mount source;
- requires isolated mount destinations to be absolute;
- rejects conflicting destinations;
- applies read-only access when access is not explicitly specified;
- applies disabled networking when it is not explicitly enabled; and
- reports the resulting effective request for dry runs and diagnostics.

There are no implicit mounts for the current directory, home directory,
credentials, SSH agent, Git configuration, or isolation-engine socket.
Configuration must name every capability that crosses the isolation boundary.

## Isolation backends

The production adapters translate an `ExecutionRequest` into Bubblewrap,
`podman run --rm`, or `docker run --rm` invocations. Bubblewrap is selected by
default. Each adapter is responsible for:

- selecting the configured rootfs or image and isolated working directory;
- translating read-only and read-write mounts;
- disabling networking by default;
- attaching the caller's standard streams and allocating a TTY when requested;
- forwarding termination as well as the backend permits; and
- returning the isolated command's exit status.

Engine-specific image names, flags, identifiers, and error details remain in
their adapters. Other backends can implement the same portable contract where
their semantics match.

The Bubblewrap adapter translates requests into unprivileged Linux
namespaces. With an explicit rootfs it mounts that prepared tree read-only.
Without one it creates a private root and mounts only conventional host system
runtime paths read-only, making `/bin/sh` and normal OS tools available without
exposing the host root, home, or current directory. It adds fresh `/proc`,
`/dev`, and `/tmp` mounts, clears the inherited host environment, and shares
the host network namespace only when networking is granted.

Tests for Driva's policy use a fake `Isolation` implementation. Each production
backend also has focused integration tests for its request translation, I/O,
exit status, and cleanup behavior.

## Relationship with Orka

Driva is independently usable from a terminal. Orka is a programmatic caller:

```text
manual user --> Driva CLI -----+
                               +--> Driva policy --> Isolation backend
Orka --------> Driva library --+
```

Orka decides what work to run and constructs the command, mounts, and network
grant. Driva validates and executes that concrete grant. It neither interprets
the command as an agent nor parses its output.

## Process lifetime

Driva runs one foreground command to completion. The caller owns that command's
lifetime: if it needs detachment, reattachment, scheduling, or restart policy,
it composes Driva with a terminal multiplexer, service manager, or job runner.
Driva does not persist process state or provide a session lifecycle API.

For example, a human can keep an interactive isolated command alive with:

```sh
tmux new-session -s work -- driva run --interactive -- COMMAND
```

The multiplexer owns terminal state and reattachment while Driva continues to
own the concrete isolation grant, standard-stream transport, exit status, and
cleanup.
