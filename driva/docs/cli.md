# Driva command-line reference

Driva runs a command in a disposable isolated environment using Podman,
Docker, or Bubblewrap with explicit, deny-by-default isolation: the command
gets no host data access and no network unless each grant is stated on the
command line or in `driva.toml`. Bubblewrap exposes the host's system runtime
read-only so basic executables are available.

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
  and `/tmp` working directory). Without a configured rootfs, Bubblewrap builds
  a private root containing only conventional read-only system runtime paths;
  the host root, home, and current directory are not selected implicitly.

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
      --config <CONFIG>     Configuration file (defaults to ./driva.toml when present)
      --template <NAME>     Apply a named execution template
      --read <MOUNT>        Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>       Add a writable mount as SOURCE or SOURCE:DESTINATION
      --no-write            Make every host mount read-only, overriding configuration and templates
      --path <DIRECTORY>    Add a host directory read-only and prepend it to the isolated PATH
      --backend <BACKEND>   Select the isolation backend
      --network             Permit networking (disabled otherwise)
      --no-network          Disable networking, overriding configuration and templates
  -i, --interactive         Allocate an interactive terminal
      --no-interactive      Disable interactivity, overriding a template
      --dry-run             Print the validated request and backend invocation without executing it
      --image <IMAGE>       Override the configured container image
      --rootfs <DIRECTORY>  Override the Bubblewrap root filesystem
      --tmpfs <DIRECTORY>   Add a private writable Bubblewrap tmpfs mount
      --workdir <WORKDIR>   Override the isolated working directory
      --env <ENVIRONMENT>   Set an environment variable as NAME=VALUE
  -h, --help                Print help
```

The trailing `<COMMAND>...` is the program and its arguments. Use `--` to
separate them from Driva's own flags when the command has flags of its own:

```sh
driva run --write . -- cargo test
driva run --read ~/.cargo/registry --write . --network -- cargo update
driva run --image rust:1.88 --workdir /workspace --write .:/workspace -- cargo build
driva run --backend bwrap --rootfs /srv/rootfs --tmpfs /home -- command
driva run --path ./tools -- project-tool
driva run --env RUST_LOG=debug -- env
```

Policy options (shared by `run`, `shell`, and `start`):

| Option | Effect |
| --- | --- |
| `--template <NAME>` | Apply a built-in or project-defined execution template. |
| `--read <MOUNT>` | Bind-mount a host path read-only. Repeatable. |
| `--write <MOUNT>` | Bind-mount a host path read-write. Repeatable. |
| `--no-write` | Make every host bind mount read-only, overriding project configuration, templates, and `--write`. |
| `--path <DIRECTORY>` | Bind-mount a host directory read-only and prepend it to the isolated `PATH`. Repeatable. |
| `--backend <BACKEND>` | Select `bwrap`, `podman`, or `docker` for this invocation. |
| `--network` | Enable networking (otherwise the container has none). |
| `--no-network` | Disable networking, overriding global configuration and templates. |
| `-i`, `--interactive` | Allocate an interactive terminal (stdin + TTY). |
| `--no-interactive` | Disable interactivity requested by a template. |
| `--dry-run` | Print the validated request and the exact backend invocation without executing anything. |
| `--image <IMAGE>` | Override the configured container image for this run (Podman/Docker only). |
| `--rootfs <DIRECTORY>` | Override the prepared root filesystem for Bubblewrap. |
| `--tmpfs <DIRECTORY>` | Add a private writable Bubblewrap tmpfs mount. Repeatable. |
| `--workdir <WORKDIR>` | Override the isolated working directory (must be absolute). |
| `--env NAME=VALUE` | Set an environment variable inside the container. Repeatable. |

Each `--path` directory must exist on the host. Relative paths are resolved
from the current host directory, and `~` is expanded from the host home
directory. Driva mounts each one read-only at its canonical host path inside
the isolation and prepends that path to `PATH` in option order. Preserving the
path allows tool managers such as Rustup to find state installed next to their
executable proxies. If configuration, a template, or `--env` supplies `PATH`,
the additions are prepended to that value; otherwise Driva retains its
conventional system path. The behavior is the same with Bubblewrap, Podman,
and Docker.

Template settings overlay the global configuration, and one-off CLI values
overlay the template. Scalar values such as backend, image, rootfs, and
working directory use CLI, then template, then configuration precedence.
Mounts, PATH additions, and tmpfs mounts accumulate in layer order;
environment values are replaced by name. Explicit `network = false` in a
template overrides enabled project networking, while `--network` and
`--no-network` provide the final CLI choice. `--no-interactive` similarly
overrides an interactive template. `--no-write` is applied after all mounts
are combined, making every host bind mount read-only regardless of its source;
private writable filesystems such as Bubblewrap tmpfs are unaffected because
they cannot modify mounted host data. Arguments after `--` are appended to a
template's command. Without a template, at least one command argument remains
required at runtime. Backend-specific combinations are validated after
resolution; for example, Docker rejects `rootfs` and Bubblewrap rejects
`image`.

### Execution templates

List effective built-in and project-defined templates with:

```console
$ driva templates
claude	Run Claude Code interactively against the current project
claude-exec	Run Claude Code non-interactively against the current project
codex	Run the host's Codex binary interactively in Bubblewrap
codex-exec	Run OpenAI Codex non-interactively against the current project
codex-runtime	Run OpenAI Codex interactively against the current project
```

The built-in `codex-runtime` template runs a pinned, prepared Codex
installation interactively. Run `driva runtime install codex@latest` or select
an exact version before using it. Installation uses Podman and
`node:22-bookworm` once to build a complete filesystem under
`~/.local/share/driva/runtimes/codex/VERSION`; normal executions expose the
active version read-only through Bubblewrap and do not use Podman.

The template passes an ephemeral Codex setting that marks the isolated project
path as trusted, avoiding the directory trust prompt before
project-scoped configuration is loaded. It disables Codex's inner sandbox and
relies on Driva's outer Bubblewrap isolation. It mounts the current directory
writable at `/driva`, enables networking, and puts `/root/.codex` on a
private writable tmpfs for disposable Codex state. It then mounts
`/etc/resolv.conf` read-only for DNS and `~/.codex/auth.json` writable at
`/root/.codex/auth.json`, allowing credential refreshes to persist. The auth
file is exposed to the selected project. The template also establishes stable
`HOME` and `TERM` values because Bubblewrap clears the inherited environment.
Use it only with trusted code. [OpenAI's authentication
documentation](https://developers.openai.com/codex/auth/) warns that
`auth.json` contains access tokens.

The built-in requires a file-backed host login at `~/.codex/auth.json`. If the
host uses an OS keyring, create a file-backed login first or define a project
replacement using another authentication scheme.

The built-in `codex` and `codex-exec` templates run the host's `codex`
executable using Bubblewrap directly, without a prepared Driva runtime.
`codex-exec` adds the `exec` subcommand for automation. They expose the host root
read-only so the executable and its shared libraries remain available, then
replace the invoking user's home directory with a private tmpfs. Only the
current project and `~/.codex` are mounted back into the sandbox. The project
is mounted at its canonical host path, while Codex state is mounted at
`/root/.codex`.
The local executable must therefore be available on the standard system `PATH`
outside the user's home directory. Like the prepared template, they trust the
isolated workspace and disable Codex's inner sandbox.

The built-in `claude` template runs `npx --yes
@anthropic-ai/claude-code@latest` interactively; `claude-exec` adds `--print`
for non-interactive use. Both use Podman with Node 22, mount the project below
its canonical host path, and enable networking. On Linux they mount
only
`~/.claude/.credentials.json` writable at
`/root/.claude/.credentials.json`, leaving other Claude configuration and
session state disposable. [Anthropic's authentication
documentation](https://docs.anthropic.com/en/docs/claude-code/iam) identifies
that file as the Linux credential store. A host Claude Code login must have
created it before using these templates.

```sh
driva run --template codex
driva run --template codex-exec -- "update the dependencies and run tests"
driva run --template codex-runtime -- --model MODEL
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
      --config <CONFIG>     Configuration file (defaults to ./driva.toml when present)
      --template <NAME>     Apply a named execution template
      --read <MOUNT>        Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>       Add a writable mount as SOURCE or SOURCE:DESTINATION
      --no-write            Make every host mount read-only, overriding configuration and templates
      --path <DIRECTORY>    Add a host directory read-only and prepend it to the isolated PATH
      --backend <BACKEND>   Select the isolation backend
      --network             Permit networking (disabled otherwise)
      --no-network          Disable networking, overriding configuration and templates
  -i, --interactive         Allocate an interactive terminal
      --no-interactive      Disable interactivity, overriding a template
      --dry-run             Print the validated request and backend invocation without executing it
      --image <IMAGE>       Override the configured container image
      --rootfs <DIRECTORY>  Override the Bubblewrap root filesystem
      --tmpfs <DIRECTORY>   Add a private writable Bubblewrap tmpfs mount
      --workdir <WORKDIR>   Override the isolated working directory
      --env <ENVIRONMENT>   Set an environment variable as NAME=VALUE
  -h, --help                Print help
```

```sh
driva shell --write .
```

## Configuration file (`driva.toml`)

Loaded from `--config <FILE>`, or `./driva.toml` when present. Defaults are
shown below; Bubblewrap's `rootfs` is optional.

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
# rootfs = "/var/lib/driva/rootfs/busybox" # optional prepared userspace
workdir = "/tmp"
executable = "bwrap"

# Zero or more pre-granted mounts. An explicit destination must be absolute;
# when omitted, the canonical source path is used as the destination.
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
path = ["~/.cargo/bin"]          # optional; read-only PATH additions
network = false
interactive = false
codex_trust_workspace = false     # optional; trusts the workspace destination

[[template.lint.workspace-mount]] # optional; at most one per template
source = "."
destination = "/workspace"      # optional; defaults to canonical source
access = "write"

[template.lint.environment]
RUST_LOG = "info"
```

Template fields are optional except that the effective command must be
non-empty. Unknown fields are rejected. `command` is an array of the executable
and its initial arguments. `backend`, `image`, `rootfs`, `tmpfs`, `workdir`,
`path`, networking, interactivity, environment, and mounts correspond to the
same per-run CLI concepts. `rootfs` overrides `[isolation.bwrap].rootfs` when
the template selects Bubblewrap, while `--rootfs` overrides both; `~` is
expanded using `$HOME` and the tree is mounted read-only.
When neither is set, Driva constructs a private root with conventional host
system runtime paths mounted read-only. It does not expose the host root, home,
current directory, or other data paths.
`tmpfs` and repeatable `--tmpfs` values replace listed rootfs directories with
private writable temporary filesystems before mounts are applied, allowing a
writable file mount inside otherwise-disposable state. A leading `~` is
expanded using the host `$HOME`; mount destinations and the working directory
may be created beneath a private tmpfs even when they are absent from the
read-only rootfs. `path` uses the same canonical, read-only mounting and PATH
ordering as repeatable `--path`.
Mount destinations are optional in configuration. When omitted, the
canonicalized host source is also used as the isolated destination.
`workspace-mount` has the same fields as a regular mount, but its resolved
destination is additionally used as the working directory. A template can
contain at most one such entry; it supersedes `workdir` when
both are set. `codex_trust_workspace` inserts a Codex configuration argument
for that destination and requires a workspace mount and a non-empty command.
Project templates appear in `driva templates`; project definitions replace
built-ins with the same name.

Bubblewrap can use `rootfs` as a prepared filesystem tree rather than pulling
an OCI image. The tree must contain `/proc`, `/dev`, `/tmp`, each configured
tmpfs mount point, and any working directory or mount destination that is not
created beneath a private tmpfs. Driva exposes it read-only. With or without a
prepared tree, Driva creates private `/proc` and `/dev` mounts and a writable
tmpfs at `/tmp`, and clears the inherited host environment. `--image` is not
supported with this backend; use `--rootfs` for a one-off prepared filesystem.

## Further reading

- [`README.md`](../README.md) — overview and quick start.
- [`DESIGN.md`](../DESIGN.md) — architecture and isolation model.
