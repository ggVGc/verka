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

Backend-specific settings, such as a Docker image, live in the selected
backend's configuration rather than the portable execution request.

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
    pub backend_reference: Option<String>,
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

The Bubblewrap adapter translates synchronous requests into unprivileged Linux
namespaces. It mounts an explicitly configured, prepared rootfs read-only,
adds fresh `/proc`, `/dev`, and `/tmp` mounts, clears the inherited host
environment, and shares the host network namespace only when networking is
granted. Requiring an explicit rootfs prevents a lightweight backend from
silently exposing the host's system tree. Bubblewrap is not a durable backend;
session lifecycle operations continue to require Podman or Docker.

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

## Stage 2: durable sessions

After synchronous execution is established, Driva may add durable sessions for
commands that must survive a detached client and be inspected or reattached
later. This remains a generic process facility; agent conversation semantics
continue to belong to Orka or another caller.

Stage 2 adds these operations while retaining `run` as the simple interface:

```text
driva start [OPTIONS] -- COMMAND [ARG...]
driva attach SESSION
driva inspect SESSION
driva wait SESSION
driva terminate SESSION
driva remove SESSION
driva list
driva recover
```

`run` may then be implemented as start, attach, wait, and remove. Existing
callers do not need to adopt the lower-level lifecycle.

### Authority and consistency

Driva must not persist a second lifecycle state machine alongside the
isolation backend. The backend is authoritative for whether its process is
created, running, exited, or absent. A Driva session record is authoritative
only for:

- the Driva session identity;
- the redacted request and effective policy;
- the selected backend and its opaque native reference; and
- immutable evidence observed from that backend.

Current status is always read from the backend:

```rust
pub struct SessionRecord {
    pub id: SessionId,
    pub backend: String,
    pub backend_reference: BackendReference,
    pub request: RedactedExecutionRequest,
    pub effective_policy: EffectivePolicy,
    pub created_at: SystemTime,
}

pub struct SessionSnapshot {
    pub record: SessionRecord,
    pub observed: ObservedProcessState,
    pub observed_at: SystemTime,
}

pub enum ObservedProcessState {
    Created,
    Running,
    Exited(ProcessExit),
    Missing,
    Unknown { error: String },
}
```

Driva may record that removal was requested or that absence was observed. It
must not claim that a resource was removed without confirming that through the
backend. Likewise, a sealed exit outcome is historical evidence, not a cached
claim about the resource's current existence.

### Durable backend capability

Not every Stage 1 backend can support durable sessions. Durability is a
separate capability rather than a requirement of `Isolation`:

```rust
pub trait DurableIsolation: Isolation {
    fn start(
        &self,
        id: &SessionId,
        request: &ExecutionRequest,
    ) -> Result<BackendReference>;

    fn find(&self, id: &SessionId) -> Result<Option<BackendReference>>;
    fn inspect(&self, reference: &BackendReference)
        -> Result<ObservedProcessState>;
    fn attach(&self, reference: &BackendReference)
        -> Result<Box<dyn ProcessConnection>>;
    fn terminate(&self, reference: &BackendReference, grace: Duration)
        -> Result<()>;
    fn remove(&self, reference: &BackendReference) -> Result<()>;
}
```

`BackendReference` is opaque outside its adapter. It may contain a Docker
container ID, a process-supervisor identity, a VM identity, or a remote
sandbox identity.

Driva chooses the session ID before starting the backend and supplies it as
backend metadata. A durable backend must be able to rediscover a managed
resource by that ID. This closes the failure window in which backend creation
succeeds but writing Driva's reference fails: recovery can find the resource
without guessing from stale lifecycle state.

A backend that cannot rediscover, inspect, attach to, and terminate a process
implements only `Isolation`. Driva must not simulate durable sessions on top
of it.

### Session interface

Programmatic callers use a lifecycle interface corresponding to the Stage 2
CLI:

```rust
pub trait SessionRunner {
    fn start(&self, request: ExecutionRequest) -> Result<StartedSession>;
    fn inspect(&self, id: &SessionId) -> Result<SessionSnapshot>;
    fn attach(&self, id: &SessionId) -> Result<Box<dyn SessionConnection>>;
    fn wait(&self, id: &SessionId) -> Result<ExecutionOutcome>;
    fn terminate(&self, id: &SessionId) -> Result<ExecutionOutcome>;
    fn remove(&self, id: &SessionId) -> Result<CleanupObservation>;
}
```

Attached and detached are client connection conditions, not persisted process
states. In particular, Driva does not infer that an arbitrary command is
waiting for input merely because no client is attached.

### Evidence and recovery

Stage 1 returns execution evidence to its caller. Stage 2 retains the session
record and append-only observations needed for later inspection. An
observation states what a backend reported at a particular time:

```rust
pub struct Observation {
    pub observed_at: SystemTime,
    pub backend_reference: BackendReference,
    pub state: ObservedProcessState,
}
```

On recovery, Driva does not replay local transitions. It finds or inspects the
backend resource, records the resulting observation, collects a final outcome
if the process exited, and retries explicitly requested cleanup. A missing
resource is reported as `Missing`; Driva does not invent an exit status.

The authority split is therefore:

```text
isolation backend  current process and resource state
Driva              request, effective grant, identity, and observations
Orka               attempt, agent transcript, and task outcome
```

### Retained output

Reattachment may require a bounded operational output log. If introduced, it
contains uninterpreted stdout and stderr events with observation sequence
numbers. It exists only to let clients catch up after detaching and is not an
agent transcript. Retention must be bounded so an absent or slow client cannot
cause unlimited storage growth. Orka remains responsible for its authoritative
transcript.

## Other deferred features

Neither stage initially includes:

- conversation or agent-context continuation;
- orchestration, retries, or task selection; or
- backend capabilities without a concrete initial use.

Remote backends and a resident service may be introduced if a Stage 2 backend
requires them, but they are implementation choices rather than part of the
portable execution semantics.
