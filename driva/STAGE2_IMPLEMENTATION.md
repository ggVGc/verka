# Driva durable sessions: production implementation plan

## Status and purpose

Driva's synchronous runner is complete enough for ordinary isolated command
execution. The first durable-session implementation establishes the Stage 2
shape: sessions can be started, attached, inspected, waited for, terminated,
removed, listed, and recovered through Podman or Docker.

That implementation is an MVP, not yet a production-complete lifecycle
facility. This document describes the work needed to make durable sessions
reliable under process crashes, concurrent clients, backend failures, and
bounded storage. It refines the authority model in [`DESIGN.md`](DESIGN.md)
without expanding Driva into an orchestrator.

The target outcome is a small, stable facility with these properties:

- the isolation backend is the sole authority for current resource state;
- Driva durably owns session identity, grants, cleanup intent, and immutable
  observations;
- every backend resource created by Driva can be discovered after a crash;
- absence is never converted into an invented exit status;
- secrets are not persisted in session metadata or diagnostics;
- attachment and retained output have explicit, bounded semantics; and
- synchronous and durable execution eventually share one lifecycle path.

## Scope boundaries

This plan includes lifecycle reliability, local persistence, retained
operational output, stable programmatic and CLI interfaces, and Podman/Docker
backend conformance.

It does not include:

- task scheduling, retry policy, or dependency management;
- agent prompts, conversation continuation, or transcript ownership;
- interpretation of stdout or stderr;
- review workflows;
- automatic network or host capability grants; or
- a remote service until a concrete backend requires one.

Orka remains responsible for attempts and authoritative agent transcripts.
Driva retained output is only a bounded byte/event stream used for operational
reattachment.

## Required invariants

All implementation work must preserve the following invariants.

### Backend authority

Driva never persists `running`, `exited`, or `missing` as mutable current
state. Those values occur only inside timestamped observations. Every
`inspect`, `list --inspect`, recovery, cleanup confirmation, and wait decision
queries the backend.

A sealed exit observation proves what the backend previously reported. It
does not prove that the backend resource still exists.

### Identity before creation

Driva selects a collision-resistant session ID before asking the backend to
create anything. Every created resource receives both:

```text
io.driva.managed=true
io.driva.session=<session-id>
```

The backend must support enumeration by the managed label and exact lookup by
session label. Native names may include the session ID for operator
convenience, but labels are the recovery contract.

### Confirmed cleanup

Local records are removed only after the backend confirms that the native
resource is absent. A failed or interrupted cleanup leaves enough durable
intent for `recover` to retry it. An engine communication failure is not
absence.

### Secret handling

Persisted requests contain command arguments, mount paths, policy flags, and
environment variable names. Environment values are never persisted. CLI JSON,
errors, tracing, and dry-run output follow the same rule.

Command arguments can themselves contain secrets. A later API may allow a
caller to mark individual arguments as sensitive, but until that exists the
documentation must explicitly warn callers that arguments are retained.

### Bounded growth

No absent or slow client can produce unlimited retained output or unbounded
metadata. Observation and output retention limits are configuration with safe
defaults. Compaction never rewrites historical facts into a false current
state.

## Target architecture

The completed lifecycle has four layers:

```text
CLI / programmatic caller
          |
          v
SessionManager -- validates requests and coordinates operations
     |                         |
     v                         v
SessionRepository         DurableIsolation
records, intent,          backend discovery, state,
observations, output      attach, wait, stop, remove
```

`SessionManager` replaces the MVP's direct coordination in `SessionRunner`.
The name change is optional, but the type must have one responsibility: apply
the lifecycle protocol using repository and backend interfaces. It must not
parse CLI arguments or invoke backend-specific commands directly.

## Data model and storage format

### Session identifiers

Replace timestamp/process/counter identifiers with 128 bits of cryptographic
randomness encoded as lowercase UUIDv4 or UUIDv7. IDs must:

- be generated without consulting the backend;
- be safe as one path component and one container label value;
- compare and serialize canonically; and
- reject noncanonical input at the CLI boundary.

UUIDv7 is preferred because it gives useful creation ordering without making
the timestamp an identity guarantee.

### Versioned session record

Each session directory contains `record.toml` with an explicit schema version:

```toml
schema_version = 1
id = "019..."
backend = "podman"
backend_reference = "opaque-native-reference"
created_at = "2026-07-12T12:34:56.123Z"
cleanup_requested = false

[request]
command = ["make", "watch"]
working_directory = "/workspace"
environment_names = ["CARGO_HOME"]
network = false
interactive = false

[policy]
# complete effective grant
```

`backend_reference` may initially be absent. This represents a record created
before backend creation or reconstructed from labels; it does not represent a
backend state. Records are written in this order:

1. generate the session ID;
2. atomically persist a prepared record without a native reference;
3. ask the backend to create the labelled resource;
4. atomically add the returned opaque reference; and
5. append the first backend observation.

If step 3 fails, the prepared record may be retained with a creation-failure
observation or removed if backend enumeration confirms no resource exists. If
step 4 fails, recovery finds the resource by its session label.

### Cleanup intent

Cleanup intent belongs in the durable record because it is Driva-owned intent,
not backend state. `remove` follows this protocol:

1. lock the session;
2. atomically set `cleanup_requested = true`;
3. request backend removal;
4. inspect the backend;
5. append the resulting observation; and
6. delete or tombstone the local session only if the result is `Missing`.

Keeping a short tombstone is preferable to immediate deletion when operators
need an audit trail. If tombstones are enabled, they contain identity,
backend, removal time, and final observation only, and expire according to a
documented retention period.

### Append-only observations

Observations use a framed format rather than concatenated TOML fragments. Two
acceptable implementations are:

- one file per monotonically numbered observation; or
- a length-prefixed append log with checksums.

One-file-per-observation is simpler and should be implemented first:

```text
sessions/<id>/observations/0000000000000001.toml
sessions/<id>/observations/0000000000000002.toml
```

Creation uses a temporary file, `fsync`, and atomic rename. Each observation
contains schema version, sequence, timestamp, backend reference, state, and a
structured error category when applicable. Sequence allocation happens under
the session lock.

Observation retention defaults to the newest 1,000 entries plus the first and
all terminal/cleanup observations. Compaction is itself recorded. Exact limits
should be configurable globally but not silently disabled.

### Atomicity and locking

Every mutable record update uses write-to-temporary, file sync, rename, and
directory sync where supported. The repository exposes a per-session
exclusive-lock guard. Operations that mutate the record, allocate an
observation sequence, terminate, or remove acquire this lock.

Read-only inspection need not hold the lock while waiting on a slow backend,
but appending its observation must allocate a sequence under the lock. Waiting
and attachment must not hold a filesystem lock for their entire duration.

The repository must detect and report corrupt records. It must never silently
replace them with defaults.

## Backend interface evolution

The durable backend contract should become:

```rust
pub trait DurableIsolation: Isolation {
    fn backend_name(&self) -> BackendName;

    fn start(
        &self,
        identity: &SessionIdentity,
        request: &ExecutionRequest,
        output: OutputCapture,
    ) -> Result<BackendReference, BackendError>;

    fn find(&self, id: &SessionId)
        -> Result<Option<BackendReference>, BackendError>;

    fn enumerate_managed(&self)
        -> Result<Vec<DiscoveredResource>, BackendError>;

    fn inspect(&self, reference: &BackendReference)
        -> Result<ObservedProcessState, BackendError>;

    fn attach(&self, reference: &BackendReference, io: ExecutionIo)
        -> Result<ProcessConnection, BackendError>;

    fn wait(&self, reference: &BackendReference)
        -> Result<ProcessExit, BackendError>;

    fn terminate(&self, reference: &BackendReference, grace: Duration)
        -> Result<(), BackendError>;

    fn remove(&self, reference: &BackendReference)
        -> Result<(), BackendError>;
}
```

`Isolation` and `DurableIsolation` may share translation helpers, but durable
support remains a separately advertised capability.

### Structured backend errors

Adapters must classify errors before the session layer sees them:

```rust
pub enum BackendErrorKind {
    NotFound,
    Unavailable,
    PermissionDenied,
    InvalidReference,
    Conflict,
    Protocol,
    Other,
}
```

Only `NotFound` maps to `ObservedProcessState::Missing`. Engine unavailable,
permission denied, malformed output, timeout, and nonzero status map to
`Unknown` observations containing a redacted diagnostic and error kind.

Podman and Docker adapters should prefer stable structured output. If an
engine offers JSON inspection, parse JSON rather than matching diagnostic
strings. Native references remain opaque outside the adapter.

### Recorded backend selection

Every operation on an existing session dispatches through the backend named in
its record. The current `driva.toml` backend selects new sessions only.

Introduce a `BackendRegistry` keyed by stable backend name. `inspect`, `wait`,
`attach`, `terminate`, `remove`, `list --inspect`, and `recover` load the
record first, then resolve that recorded backend. If it is unavailable, report
`Unknown(backend_unconfigured)` without trying a different engine.

## Recovery protocol

Recovery reconciles records and backend resources; it does not replay local
state transitions.

For each configured durable backend:

1. enumerate all resources labelled `io.driva.managed=true`;
2. validate and group them by Driva session ID;
3. report duplicate resources for one session as conflicts and do not choose
   one automatically;
4. load the matching local record when present;
5. if the record has no reference or has a stale reference, update it to the
   uniquely discovered reference;
6. if no record exists, create a quarantined recovered record containing the
   discoverable identity and backend reference;
7. inspect the resource and append an observation;
8. collect exit evidence when the backend reports an exited resource; and
9. retry removal when `cleanup_requested` is true.

Then inspect every local record not found during enumeration. Exact lookup by
session ID protects against incomplete engine enumeration. A confirmed miss
produces `Missing`; an enumeration or lookup error produces `Unknown`.

A recovered orphan cannot safely reconstruct its original command, mounts, or
environment metadata from container inspection. Its record is explicitly
marked `metadata = "incomplete"`. It may be inspected, terminated, or removed,
but callers must not treat it as complete execution evidence.

`recover` is idempotent. Running it twice without backend changes may append a
new observation but must not duplicate records, resources, or cleanup actions.

## Wait, exit evidence, and termination

`wait` asks the backend to wait and return the native exit result. It then
inspects the resource and seals execution evidence. If waiting reports that
the resource is missing and no earlier exit observation exists, the operation
returns `Missing`, not exit code 1 or 128.

`terminate` is defined as:

1. request graceful termination with the configured grace duration;
2. wait for a terminal backend result;
3. append the terminal observation; and
4. return execution evidence without automatically removing the resource.

Forced termination after the grace period is backend policy exposed through a
portable option only if Podman and Docker can provide equivalent semantics.
Signal-derived exits should retain the signal when the platform exposes it;
the CLI may still map them to a conventional shell exit code.

Repeated `wait` and `terminate` calls must be idempotent at the Driva layer.
Previously sealed exit evidence can be returned as historical evidence only
after an inspection confirms whether the resource is exited or missing. The
response distinguishes historical outcome from current resource state.

## Attachment and retained output

### Attachment contract

Attachment is a connection condition, never a persisted process state.

The initial production contract should allow multiple output readers but at
most one stdin owner. A second client requesting stdin receives a conflict;
read-only attachment remains possible. Client disconnect does not terminate
the command. Signals received by an attached foreground client are forwarded
when supported and documented per backend.

TTY and non-TTY sessions have different constraints:

- TTY sessions retain the backend's combined terminal byte stream.
- Non-TTY sessions retain separate stdout and stderr events.
- Terminal resizing is forwarded only for interactive TTY attachments.
- Attaching never implies that the command is waiting for input.

### Output event model

Retained output uses uninterpreted events:

```rust
pub struct OutputEvent {
    pub sequence: u64,
    pub observed_at: SystemTime,
    pub stream: OutputStream,
    pub bytes: Vec<u8>,
}

pub enum OutputStream {
    Stdout,
    Stderr,
    Terminal,
}
```

Bytes are not assumed to be UTF-8. The CLI writes them directly to the
corresponding stream. JSON output base64-encodes byte payloads.

Default retention should be both byte- and age-bounded, for example 16 MiB per
session and seven days. The exact defaults should be decided with Orka's
expected detach duration. When old output is discarded, readers receive a
`Gap { first_available_sequence }` marker rather than silently assuming a
complete stream.

### Capture implementation

Direct `docker attach` or `podman attach` alone cannot replay output reliably.
Implement one of these strategies after a short backend spike:

1. use engine logs with timestamps/cursors for catch-up, then attach for live
   I/O; or
2. route container output through a small Driva-owned capture process that
   writes the bounded event log and fans out to clients.

Prefer engine logs if both backends can preserve stream identity, byte
fidelity, ordering, and a race-free transition to live following. Otherwise a
resident capture helper is justified, but it must remain a generic byte
transport rather than an orchestration service.

## CLI and programmatic interface

Human-readable output remains concise. Every inspection-oriented command also
supports `--json` with a versioned schema:

```text
driva start --json -- COMMAND...
driva inspect --json SESSION
driva list --json [--inspect]
driva recover --json
driva wait --json SESSION
driva remove --json SESSION
```

`start --json` returns the session ID, recorded backend, and native reference
only if exposing the reference is explicitly part of the public schema.
`inspect` returns record metadata, current observation, observation timestamp,
and metadata completeness. Stable state strings are `created`, `running`,
`exited`, `missing`, and `unknown`.

`list` should default to cheap local records and expose `--inspect` for live
backend queries. This prevents a large session collection from unexpectedly
making many slow engine calls. Human output clearly labels cached record data
versus live observations.

Commands return documented exit categories:

- command exit status for `run`, `attach`, `wait`, and `terminate` when known;
- zero for successful metadata and cleanup operations;
- a stable Driva operational failure code when backend state is unknown; and
- a distinct not-found code if useful to scripts.

The Rust API returns typed errors and must not require callers to parse CLI
strings.

## Implementing `run` through sessions

After lifecycle conformance and output retention are stable, implement `run`
as:

1. start a durable session;
2. attach with inherited I/O;
3. wait for sealed exit evidence;
4. request removal;
5. confirm absence; and
6. return the command exit status.

If cleanup fails after an exit is known, Driva must preserve cleanup intent and
report the cleanup problem without losing the command outcome. The API should
represent both facts rather than replacing one with the other.

Retain an explicitly disposable backend fast path only if benchmarks show that
durable lifecycle overhead is material. Having two semantic paths carries a
high correctness cost and should not be the default.

## Implementation phases

### Phase 1: lifecycle correctness

Deliver:

- structured backend errors;
- correct `Missing` versus `Unknown` classification;
- a backend registry using the backend recorded per session;
- UUID session identities and both required resource labels;
- prepared records and atomic reference updates;
- durable cleanup intent; and
- atomic, sequenced observation files.

Acceptance criteria:

- daemon unavailability never appears as `Missing`;
- changing the configured default backend does not break existing sessions;
- interruption after backend creation is recoverable by session label;
- interrupted removal is retried by recovery; and
- concurrent inspections cannot corrupt or reuse observation sequences.

### Phase 2: full reconciliation

Deliver:

- `enumerate_managed` for Podman and Docker;
- orphan reconstruction and quarantine;
- duplicate-resource conflict reporting;
- idempotent recovery across all configured backends; and
- optional tombstone retention.

Acceptance criteria:

- a labelled container with no local record appears in recovery output;
- stale native references are repaired only from a unique label match;
- duplicate matches cause a visible conflict and no destructive action; and
- recovery never invents missing request metadata or an exit result.

### Phase 3: retained output and attachment

Deliver:

- the output event format and bounded store;
- backend capture/follow support;
- catch-up followed by race-free live output;
- gap markers after retention truncation;
- explicit stdin ownership; and
- TTY resize and signal behavior where supported.

Acceptance criteria:

- a client attaching late receives retained output in order and then live
  output without duplication;
- binary output survives unchanged;
- stdout and stderr identity is preserved for non-TTY sessions;
- a slow or absent client cannot exceed configured retention; and
- disconnecting all clients leaves the command running.

### Phase 4: stable external interface

Deliver:

- versioned JSON for all lifecycle commands;
- documented CLI exit behavior;
- typed Rust errors and snapshots;
- storage schema migration tooling; and
- operator diagnostics for corrupt records and backend conflicts.

Acceptance criteria:

- golden tests lock down JSON schemas and human output;
- the previous storage schema migrates without exposing secret values;
- malformed records are reported and preserved for diagnosis; and
- programmatic callers do not parse display strings.

### Phase 5: unify synchronous execution

Deliver:

- `run` implemented with the durable lifecycle;
- combined command-outcome and cleanup reporting; and
- compatibility tests for all existing Stage 1 behavior.

Acceptance criteria:

- existing `run` policy, I/O, dry-run, and exit-status tests remain valid;
- an interrupted `run` can be found by `recover`;
- cleanup failures leave durable retry intent; and
- no implicit host or network capabilities are introduced.

## Test strategy

### Unit and property tests

Use fake repositories and backends to cover every failure boundary:

- failure before and after prepared-record persistence;
- backend creation success followed by reference-write failure;
- every backend error classification;
- termination races with natural exit;
- removal success followed by inspect failure;
- sequence allocation under concurrency;
- corrupt and partially written records;
- cleanup retry idempotence;
- retention truncation and gap generation; and
- redaction of environment values in every serialized/error surface.

Property tests should verify SessionId parsing/round trips, observation
sequence monotonicity, and storage migration idempotence.

### Backend contract tests

Run the same durable conformance suite against Podman and Docker. The suite
creates uniquely labelled short-lived containers and verifies:

- start and exact rediscovery;
- enumeration by managed label;
- created/running/exited/missing inspection;
- exact exit codes and signal outcomes;
- attach and detach behavior;
- stdin/stdout/stderr transport;
- graceful and forced termination;
- remove and confirmed absence;
- daemon-unavailable classification; and
- cleanup after every test, including failed assertions.

Tests requiring an engine remain opt-in locally and run in dedicated CI jobs.
Unit tests must not require a container engine.

### Crash tests

Add a small test harness with failpoints after each lifecycle persistence or
backend step. Kill the Driva process at the failpoint, run recovery, and assert
the invariant rather than a particular implementation transition. Important
cases are creation, terminal evidence sealing, cleanup intent, and record
deletion.

### Compatibility tests

Keep Stage 1 request translation and deny-by-default policy tests. Add golden
tests for old record migrations and CLI JSON. A new schema version requires at
least one fixture from every previously released version.

## Observability and operations

Diagnostics should include session ID, backend name, operation, and structured
error category. Do not include environment values or retained output bytes.
Native references may be logged at debug level if they are not credentials.

Useful operational commands are:

```text
driva inspect SESSION --json
driva list --inspect
driva recover --dry-run
driva recover
driva doctor
```

`recover --dry-run` reports reconciliation and cleanup actions without
mutating records or backend resources. `doctor`, if added, checks state
directory permissions/locking and configured engine availability; it does not
alter sessions.

Metrics are unnecessary for the standalone CLI. If a resident service is
introduced later, it should expose counts and durations without session
commands, environment values, mount paths, or output content as metric labels.

## Migration from the MVP

The current unversioned `record.toml` becomes schema version 0. On first
mutation, or through an explicit `driva migrate`, Driva:

1. acquires the session lock;
2. parses the old record strictly;
3. writes a backup in the session directory;
4. converts millisecond timestamps to the canonical timestamp format;
5. sets cleanup intent to false;
6. converts existing appended observations into sequenced files;
7. writes schema version 1 atomically; and
8. leaves the backup until a later successful recovery confirms readability.

Migration cannot infer metadata that was never stored. Such fields are marked
unknown rather than defaulted. Migration never queries a different backend
from the one recorded in the session.

## Definition of feature completeness

Driva durable sessions are feature-complete when all five phases meet their
acceptance criteria on both supported backends and the following statement is
true:

> For any interruption point, Driva can either reconcile a managed backend
> resource to durable metadata, report a structured uncertainty requiring
> operator action, or confirm its absence, without inventing lifecycle state or
> leaking persisted secrets.

After that point, new features should require a concrete consumer and preserve
Driva's generic command-runner boundary. Remote backends or a resident service
are reasonable future capabilities; orchestration and agent semantics are not.
