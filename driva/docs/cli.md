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
  templates  List built-in and project-defined execution templates
  runtime    Manage prepared read-only runtimes for Bubblewrap templates
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

Usage: driva run [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...  

Options:
      --config <CONFIG>    Configuration file (defaults to ./driva.toml when present)
      --template <NAME>    Apply a named execution template
      --read <MOUNT>       Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>      Add a writable mount as SOURCE or SOURCE:DESTINATION
      --network            Permit networking (disabled otherwise)
      --no-network         Disable networking, overriding configuration and templates
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
| `--template <NAME>` | Apply a built-in or project-defined execution template. |
| `--read <MOUNT>` | Bind-mount a host path read-only. Repeatable. |
| `--write <MOUNT>` | Bind-mount a host path read-write. Repeatable. |
| `--network` | Enable networking (otherwise the container has none). |
| `--no-network` | Disable networking, overriding global configuration and templates. |
| `-i`, `--interactive` | Allocate an interactive terminal (stdin + TTY). |
| `--dry-run` | Print the validated request and the exact backend invocation without executing anything. |
| `--image <IMAGE>` | Override the configured container image for this run (Podman/Docker only). |
| `--workdir <WORKDIR>` | Override the isolated working directory (must be absolute). |
| `--env NAME=VALUE` | Set an environment variable inside the container. Repeatable. |

Template settings overlay the global configuration, and one-off CLI values
overlay the template. Mounts are appended in global, template, then CLI order;
environment values use the same precedence. Networking is enabled if any layer
enables it unless the CLI specifies `--no-network`; `--network` and
`--no-network` are mutually exclusive. Interactivity is enabled if any layer
enables it. Arguments after `--` are appended to a template's command. Without
a template, at least one command argument remains required at runtime.

### Execution templates

List effective built-in and project-defined templates with:

```console
$ driva templates
claude	Run Claude Code interactively against the current project
claude-exec	Run Claude Code non-interactively against the current project
codex	Run OpenAI Codex interactively against the current project
codex-exec	Run OpenAI Codex non-interactively against the current project
codex-local	Run the host's Codex binary interactively in Bubblewrap
```

The built-in `codex` template runs a pinned, prepared Codex installation
interactively; `codex-exec` inserts the `exec` subcommand for automation. Run
`driva runtime install codex@latest` or select an exact version before using
either template. Installation uses Podman and `node:22-bookworm` once to build
a complete filesystem under
`~/.local/share/driva/runtimes/codex/VERSION`; normal executions expose the
active version read-only through Bubblewrap and do not use Podman.

Both templates seed an ephemeral user-level Codex configuration that marks
`/workspace` as trusted, avoiding the directory trust prompt before
project-scoped configuration is loaded. They disable Codex's inner sandbox and
rely on Driva's outer Bubblewrap isolation. They mount the current directory
writable at `/workspace`, enable networking, and put `/root/.codex` on a private
writable tmpfs for disposable Codex state. They then mount
`/etc/resolv.conf` read-only for DNS and `~/.codex/auth.json` writable at
`/root/.codex/auth.json`, allowing credential refreshes to persist. The auth
file is exposed to the selected project. The templates also establish stable
`HOME` and `TERM` values because Bubblewrap clears the inherited environment.
Use them only with trusted code. [OpenAI's authentication
documentation](https://developers.openai.com/codex/auth/) warns that
`auth.json` contains access tokens.

The built-in requires a file-backed host login at `~/.codex/auth.json`. If the
host uses an OS keyring, create a file-backed login first or define a project
replacement using another authentication scheme.

The built-in `codex-local` template runs the host's `codex` executable using
Bubblewrap directly, without a prepared Driva runtime. It exposes the host root
read-only so the executable and its shared libraries remain available, then
replaces the invoking user's home directory with a private tmpfs. Only the
current project and `~/.codex/auth.json` are mounted back into the sandbox, at
`/tmp/workspace` and the disposable `/root` respectively. The local
executable must therefore be available on the standard system `PATH` outside
the user's home directory. Like the prepared templates, it trusts the isolated
workspace and disables Codex's inner sandbox.

The built-in `claude` template runs `npx --yes
@anthropic-ai/claude-code@latest` interactively; `claude-exec` adds `--print`
for non-interactive use. Both use Podman with Node 22, mount the project at
`/workspace`, and enable networking. On Linux they mount only
`~/.claude/.credentials.json` writable at
`/root/.claude/.credentials.json`, leaving other Claude configuration and
session state disposable. [Anthropic's authentication
documentation](https://docs.anthropic.com/en/docs/claude-code/iam) identifies
that file as the Linux credential store. A host Claude Code login must have
created it before using these templates.

```sh
driva run --template codex
driva run --template codex-exec -- "update the dependencies and run tests"
driva run --template codex -- --model MODEL
driva run --template codex-local
driva run --template claude
driva run --template claude-exec -- "update the dependencies and run tests"
```

A `[template.<name>]` entry with a built-in name completely replaces that
built-in, allowing images and package versions to be pinned locally.

### Prepared runtimes

```console
$ driva runtime --help
Manage prepared read-only runtimes for Bubblewrap templates

Usage: driva runtime [OPTIONS] <COMMAND>

Commands:
  install  Build and install a runtime such as codex@latest
  list     List installed runtime versions
  remove   Remove an installed runtime version
  help     Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

Install the current Codex release before the first Bubblewrap-backed Codex run:

```sh
driva runtime install codex@latest
driva runtime list
```

The installer uses the configured Podman executable to create a temporary
container, resolves npm's `latest` tag, installs that exact `@openai/codex`
version, exports and extracts its filesystem, and removes the build container.
The artifact is stored under the resolved concrete version, never under
`latest`, so installed runtimes remain immutable and reproducible. Publication
is atomic. Reinstalling an existing concrete version makes it current without
rebuilding it; installing another version atomically moves the `current` link.
A custom preparation image can be selected with `--image`.

```console
$ driva runtime install --help
Build and install a runtime such as codex@latest

Usage: driva runtime install [OPTIONS] <RUNTIME>

Arguments:
  <RUNTIME>  Runtime selector in NAME@VERSION form; VERSION may be latest

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
      --image <IMAGE>    Container image used to prepare the runtime filesystem [default: docker.io/library/node:22-bookworm]
  -h, --help             Print help
```

```sh
driva runtime install codex@latest --image registry.example/node:22
driva runtime remove codex@0.144.3
```

Removing the current version also removes the `current` link. Install or
reinstall another version before running the built-in Codex templates. Removal
requires the concrete version shown by `driva runtime list`; `codex@latest` is
accepted only by `runtime install`.

```console
$ driva runtime list --help
List installed runtime versions

Usage: driva runtime list [OPTIONS]

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

```console
$ driva runtime remove --help
Remove an installed runtime version

Usage: driva runtime remove [OPTIONS] <RUNTIME>

Arguments:
  <RUNTIME>  Pinned runtime in NAME@VERSION form

Options:
      --config <CONFIG>  Configuration file (defaults to ./driva.toml when present)
  -h, --help             Print help
```

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
      --template <NAME>    Apply a named execution template
      --read <MOUNT>       Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>      Add a writable mount as SOURCE or SOURCE:DESTINATION
      --network            Permit networking (disabled otherwise)
      --no-network         Disable networking, overriding configuration and templates
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

Usage: driva start [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...  

Options:
      --config <CONFIG>    Configuration file (defaults to ./driva.toml when present)
      --template <NAME>    Apply a named execution template
      --read <MOUNT>       Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>      Add a writable mount as SOURCE or SOURCE:DESTINATION
      --network            Permit networking (disabled otherwise)
      --no-network         Disable networking, overriding configuration and templates
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

[template.lint]
description = "Run the Rust linter"
command = ["cargo", "clippy"]
backend = "podman"               # optional: "bwrap", "podman", or "docker"
image = "rust:1.88"              # optional; Podman/Docker only
rootfs = "/srv/driva/rootfs/rust" # optional; Bubblewrap only
tmpfs = ["/root/.cache"]          # optional; Bubblewrap only
workdir = "/workspace"           # optional
network = false
interactive = false

[template.lint.environment]
RUST_LOG = "info"

[[template.lint.mount]]
source = "."
destination = "/workspace"
access = "write"
```

Template fields are optional except that the effective command must be
non-empty. `command` is an array of the executable and its initial arguments.
`rootfs` overrides `[isolation.bwrap].rootfs` when the template selects
Bubblewrap; `~` is expanded using `$HOME` and the tree is mounted read-only.
`tmpfs` replaces listed rootfs directories with private writable temporary
filesystems before template mounts are applied, allowing a writable file mount
inside otherwise-disposable state. A leading `~` is expanded using the host
`$HOME`; mount destinations and the working directory may be created beneath a
private tmpfs even when they are absent from the read-only rootfs.
Template mounts use the same shape and validation as global `[[mount]]`
entries. Project templates appear in `driva templates`; project definitions
replace built-ins with the same name.

Bubblewrap uses `rootfs` as a prepared filesystem tree rather than pulling an
OCI image. The tree must contain `/proc`, `/dev`, `/tmp`, each configured tmpfs
mount point, and any working directory or mount destination that is not created
beneath a private tmpfs. Driva exposes it read-only, creates private `/proc` and
`/dev` mounts and a writable tmpfs at `/tmp`, and clears the inherited host
environment. `--image` and durable session commands are not supported with
this backend.

## Further reading

- [`README.md`](../README.md) — overview and quick start.
- [`DESIGN.md`](../DESIGN.md) — architecture and isolation model.
- [`STAGE2_IMPLEMENTATION.md`](../STAGE2_IMPLEMENTATION.md) — durable-session
  lifecycle invariants and storage protocol.
