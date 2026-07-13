# Driva command-line reference

Driva runs a command in a disposable isolated environment using Podman,
Docker, or Bubblewrap with explicit, deny-by-default isolation: the command
gets no host filesystem access and no network unless each grant is stated on
the command line or in `driva.toml`.

The `console` blocks below are verified against the compiled binary by
`tests/cli_docs.rs`. If a flag or help text changes, that test fails; run
`DRIVA_UPDATE_DOCS=1 cargo test --test cli_docs` to regenerate the blocks,
then review and update the surrounding prose.

## Invocation

```console
$ driva --help
Run a command with explicit, deny-by-default isolation

Usage: driva [OPTIONS] <COMMAND>

Commands:
  run        Run a command in a disposable isolated environment
  shell      Open /bin/sh in a disposable isolated environment
  start      Start a durable isolated session and print its id
  attach     Attach the terminal to a durable session
  inspect    Inspect backend-authoritative session state
  wait       Wait for a session and return its exit status
  terminate  Gracefully terminate a session and return its exit status
  remove     Remove a session resource and its local record after confirming absence
  list       List recorded sessions and their current backend states
  recover    Rediscover and inspect recorded sessions
  help       Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Every subcommand accepts the global option:

- `--config <CONFIG>` — configuration file to load. When omitted, Driva uses
  `./driva.toml` if it exists, otherwise built-in defaults (Bubblewrap backend
  and `/tmp` working directory). Bubblewrap still requires an explicitly
  configured rootfs; the host root is never selected implicitly.

On any internal error Driva prints `driva: <error>` to stderr and exits with
status 1. Commands that proxy an isolated process exit with that process's
exit code; a process killed by a signal is reported as exit code 128.

## Disposable execution

### `driva run`

Runs a command in a fresh container that is removed when the command exits.

```console
$ driva run --help
Run a command in a disposable isolated environment

Usage: driva run [OPTIONS] <COMMAND>...

Arguments:
  <COMMAND>...  

Options:
      --config <CONFIG>    Configuration file (defaults to ./driva.toml when present)
      --read <MOUNT>       Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>      Add a writable mount as SOURCE or SOURCE:DESTINATION
      --network            Permit networking (disabled otherwise)
  -i, --interactive        Allocate an interactive terminal
      --dry-run            Print the validated request and backend invocation without executing it
      --image <IMAGE>      Override the configured container image
      --workdir <WORKDIR>  Override the isolated working directory
      --env <ENVIRONMENT>  Set an environment variable as NAME=VALUE
  -h, --help               Print help
```

The trailing `<COMMAND>...` is the program and its arguments. Use `--` to
separate them from Driva's own flags when the command has flags of its own:

```sh
driva run --write . -- cargo test
driva run --read ~/.cargo/registry --write . --network -- cargo update
driva run --image rust:1.88 --workdir /workspace --write .:/workspace -- cargo build
driva run --env RUST_LOG=debug -- env
```

Policy options (shared by `run`, `shell`, and `start`):

| Option | Effect |
| --- | --- |
| `--read <MOUNT>` | Bind-mount a host path read-only. Repeatable. |
| `--write <MOUNT>` | Bind-mount a host path read-write. Repeatable. |
| `--network` | Enable networking (otherwise the container has none). |
| `-i`, `--interactive` | Allocate an interactive terminal (stdin + TTY). |
| `--dry-run` | Print the validated request and the exact backend invocation without executing anything. |
| `--image <IMAGE>` | Override the configured container image for this run (Podman/Docker only). |
| `--workdir <WORKDIR>` | Override the isolated working directory (must be absolute). |
| `--env NAME=VALUE` | Set an environment variable inside the container. Repeatable. |

Command-line mounts are appended after any `[[mount]]` entries from the
configuration; `--env` values override configured `[environment]` entries
with the same name; `--network` is OR-ed with `[network] enabled`.

### Mount grammar

`--read` and `--write` accept `SOURCE` or `SOURCE:DESTINATION`.

- The source must exist on the host; it is canonicalized (symlinks resolved)
  before use. `~` and `~/...` are expanded using `$HOME`.
- With an explicit `:DESTINATION`, the destination must be an absolute path
  inside the container.
- Without a destination:
  - `.` is mounted at the container working directory,
  - another relative source is mounted below the working directory
    (`--read data` → `<workdir>/data`),
  - an absolute source is mounted at the same path inside the container.
- Two mounts may not target the same destination.

### `driva shell`

Opens `/bin/sh` in a disposable container. Identical policy options to
`run`; `--interactive` is implied.

```console
$ driva shell --help
Open /bin/sh in a disposable isolated environment

Usage: driva shell [OPTIONS]

Options:
      --config <CONFIG>    Configuration file (defaults to ./driva.toml when present)
      --read <MOUNT>       Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>      Add a writable mount as SOURCE or SOURCE:DESTINATION
      --network            Permit networking (disabled otherwise)
  -i, --interactive        Allocate an interactive terminal
      --dry-run            Print the validated request and backend invocation without executing it
      --image <IMAGE>      Override the configured container image
      --workdir <WORKDIR>  Override the isolated working directory
      --env <ENVIRONMENT>  Set an environment variable as NAME=VALUE
  -h, --help               Print help
```

```sh
driva shell --write .
```

## Durable sessions

A durable session is a detached container that keeps running after Driva
exits. `start` prints the session id; the remaining subcommands take that id.
Session records (grants and environment variable *names*, never values) are
stored under `$DRIVA_STATE_DIR`, `$XDG_STATE_HOME/driva`, or
`~/.local/state/driva`, in that order of preference. Process state is always
queried from the container backend — a missing container is never reported
as a successful exit.

Durable sessions require Podman or Docker. The Bubblewrap backend supports
only disposable `run` and `shell` execution.

```sh
id=$(driva start --write . -- make watch)
driva inspect "$id"
driva attach "$id"
driva wait "$id"
driva terminate "$id" --grace 10
driva remove "$id"
driva list
driva recover
```

### `driva start`

```console
$ driva start --help
Start a durable isolated session and print its id

Usage: driva start [OPTIONS] <COMMAND>...

Arguments:
  <COMMAND>...  

Options:
      --config <CONFIG>    Configuration file (defaults to ./driva.toml when present)
      --read <MOUNT>       Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>      Add a writable mount as SOURCE or SOURCE:DESTINATION
      --network            Permit networking (disabled otherwise)
  -i, --interactive        Allocate an interactive terminal
      --dry-run            Print the validated request and backend invocation without executing it
      --image <IMAGE>      Override the configured container image
      --workdir <WORKDIR>  Override the isolated working directory
      --env <ENVIRONMENT>  Set an environment variable as NAME=VALUE
  -h, --help               Print help
```

Accepts the same policy options as `run` and prints the new session id on
stdout. With `--dry-run` it prints the backend invocation instead of
starting anything.

### `driva attach`

```console
$ driva attach --help
Attach the terminal to a durable session

Usage: driva attach [OPTIONS] <SESSION>

Arguments:
  <SESSION>  

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Connects the current terminal to the session's process and exits with the
session's exit code when it finishes.

### `driva inspect`

```console
$ driva inspect --help
Inspect backend-authoritative session state

Usage: driva inspect [OPTIONS] <SESSION>

Arguments:
  <SESSION>  

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Prints `<id> <backend> <observed-state>`, where the state comes from the
container backend, not the local record.

### `driva wait`

```console
$ driva wait --help
Wait for a session and return its exit status

Usage: driva wait [OPTIONS] <SESSION>

Arguments:
  <SESSION>  

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Blocks until the session finishes and exits with the session's exit code.

### `driva terminate`

```console
$ driva terminate --help
Gracefully terminate a session and return its exit status

Usage: driva terminate [OPTIONS] <SESSION>

Arguments:
  <SESSION>  

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
      --grace <GRACE>    [default: 10]
  -h, --help             Print help
```

Stops the session gracefully, waiting up to `--grace` seconds (default 10)
before forcing termination. Prints the exit status and exits with the
session's exit code. The session record and backend resource remain until
`driva remove` is run.

### `driva remove`

```console
$ driva remove --help
Remove a session resource and its local record after confirming absence

Usage: driva remove [OPTIONS] <SESSION>

Arguments:
  <SESSION>  

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Deletes the backend resource and the local record, but only after confirming
the resource is actually gone; it fails if the backend still reports the
resource as present.

### `driva list`

```console
$ driva list --help
List recorded sessions and their current backend states

Usage: driva list [OPTIONS]

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Prints one tab-separated line per recorded session:
`<id> <backend> <observed-state>`, with a trailing note for sessions that
were recovered with incomplete metadata.

### `driva recover`

```console
$ driva recover --help
Rediscover and inspect recorded sessions

Usage: driva recover [OPTIONS]

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Rediscovers sessions from the container backend (containers are labelled
with their Driva session id), reconciles them with local records, and prints
them in the same format as `list`.

## Configuration file (`driva.toml`)

Loaded from `--config <FILE>`, or `./driva.toml` when present. Defaults are
shown below; Bubblewrap's `rootfs` is required for execution.

```toml
[isolation]
backend = "bwrap"                 # or "podman" or "docker"

[isolation.podman]
image = "docker.io/library/busybox:latest"
workdir = "/tmp"
executable = "podman"

[isolation.docker]
image = "docker.io/library/busybox:latest"
workdir = "/tmp"
executable = "docker"

[isolation.bwrap]
rootfs = "/var/lib/driva/rootfs/busybox" # required when backend = "bwrap"
workdir = "/tmp"
executable = "bwrap"

# Zero or more pre-granted mounts. destination is required and absolute.
[[mount]]
source = "."
destination = "/workspace"
access = "write"                  # "read"/"ro" or "write"/"rw"; default read-only

[network]
enabled = false                   # --network also enables it per run

[environment]
# NAME = "value" pairs set inside every sandbox; --env overrides per run.
```

Bubblewrap uses `rootfs` as a prepared filesystem tree rather than pulling an
OCI image. The tree must contain `/proc`, `/dev`, `/tmp`, the configured
working directory, and every mount destination. Driva exposes it read-only,
creates private `/proc` and `/dev` mounts and a writable tmpfs at `/tmp`, and
clears the inherited host environment. `--image` and durable session commands
are not supported with this backend.

## Further reading

- [`README.md`](../README.md) — overview and quick start.
- [`DESIGN.md`](../DESIGN.md) — architecture and isolation model.
- [`STAGE2_IMPLEMENTATION.md`](../STAGE2_IMPLEMENTATION.md) — durable-session
  lifecycle invariants and storage protocol.
