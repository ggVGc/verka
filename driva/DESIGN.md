# Driva design

## Purpose

Driva is a small standalone application for running a command in an isolated
environment. It provides a convenient, reusable interface for both manual use
and programmatic callers such as Orka.

Driva does not implement isolation itself. Its core validates a portable
execution request and delegates it to an isolation backend. Bubblewrap is the
backend for lightweight synchronous Linux execution. Backend-specific concepts
are not part of the core interface.

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
configuration, one or more templates, and command-line overrides resolve
through one launch layer before the selected adapter is constructed. Scalar
precedence is CLI, then later templates, then earlier templates, then project
configuration.

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

pub enum Mount {
    Bind {
        source: PathBuf,
        destination: PathBuf,
        access: MountAccess,
    },
    Temporary {
        destination: PathBuf,
    },
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
is that the trait describes an isolated process rather than exposing
backend-specific operations such as creating or removing containers.

## Validation and policy

Before invoking a backend, Driva:

- resolves and validates every host mount source;
- requires isolated mount destinations to be absolute;
- rejects conflicting destinations;
- creates temporary mounts as empty writable filesystems for one execution;
- applies read-only access when access is not explicitly specified;
- applies disabled networking when it is not explicitly enabled; and
- reports the resulting effective request for dry runs and diagnostics.

When no working directory is selected by the CLI, template, or backend
configuration, Driva implicitly mounts the current directory writable at its
canonical same-path destination and uses it as the workspace. There are no
implicit mounts for the home directory, credentials, SSH agent, Git
configuration, or isolation-engine socket. Configuration must name every
other capability that crosses the isolation boundary.

## Isolation backends

The production adapter translates an `ExecutionRequest` into a Bubblewrap
invocation. Each adapter is responsible for:

- selecting the configured rootfs and isolated working directory;
- translating read-only, read-write, and temporary mounts;
- disabling networking by default;
- attaching the caller's standard streams and allocating a TTY when requested;
- forwarding termination as well as the backend permits; and
- returning the isolated command's exit status.

Engine-specific flags, identifiers, and error details remain in their
adapters. Other backends can implement the same portable contract where their
semantics match.

The Bubblewrap adapter translates requests into unprivileged Linux
namespaces. With an explicit rootfs it mounts that prepared tree read-only.
Without one it creates a private root and builds it from the configured *base*
(below), making `/bin/sh` and normal OS tools available without exposing the
host root or home directory. The launch layer adds the default workspace mount
when applicable. Bubblewrap adds fresh `/proc`, `/dev`, and `/tmp` mounts,
clears the inherited host environment, and shares the host network namespace
only when networking is granted.

## The base system

A private root starts as an empty filesystem, so a command in it cannot run
until the host's loader, libraries, and system files are there. That floor is
the *base*, and it is deliberately a different concept from a mount: a mount
grants access to the operator's own data and is a choice, while the base is
what any program needs in order to run at all and is not.

The base is a list of named **capabilities** rather than one list of paths,
because the two questions differ in kind. "This sandbox must be able to resolve
host names" is portable; `/run/systemd/resolve` is one machine's answer to it.
Separating them is what lets a single set of built-ins work across
distributions, and lets an operator state the difference where it does not:

- a capability names host paths, each `optional` or not, in one of four modes
  (`auto`, `bind`, `symlink`, `follow`) that say what to do about a host
  symlink;
- it may forward named host environment variables — a proxy, an overridden
  certificate bundle — which is the only way a value a program cannot find on
  disk crosses the boundary;
- it may declare a **probe**, a closed-set check (`resolve`, `connect`, `run`)
  that answers whether it actually works here;
- it may `suggest` paths worth reporting when the probe fails.

The built-ins (`core`, `identity`, `certificates`, `dns`, `timezone`) are
embedded TOML deserialized through the same schema as a project's own, and a
project definition of a name replaces the built-in. `driva doctor` builds a
sandbox from each included capability and probes it, so a host whose layout the
built-ins do not describe produces a precise report and a configuration
suggestion rather than a failure inside whatever was launched.

Three properties hold this together, and they are the reason the mechanism is
worth its size:

- **Nothing is implied.** A capability is a declaration in configuration; a
  suggestion is printed, never applied. Discovery reports, it does not grant.
- **A gap is loud where it is cheap.** A path that is not `optional` and not on
  the host fails resolution, naming the capability. Probes catch what a path
  list cannot state.
- **What is shown is what is bound.** `resolve_base` returns the entries the
  adapter lays down, so a host that displays its sandboxes (Styra does) cannot
  drift from what they hold.

A template may `capability = [...]` to state what its command requires, but
never defines one: what a capability means on a host is configuration's
business, so selecting a template cannot widen the root by itself.

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
