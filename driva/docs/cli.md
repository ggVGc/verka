# Driva command-line reference

Driva runs a command in a disposable isolated environment using Bubblewrap
with explicit, deny-by-default isolation: the command gets no host data access
and no network unless each grant is stated on the command line or in
`driva.toml`. Bubblewrap exposes the host's system runtime read-only so basic
executables are available.

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
  and no configured working directory). When the effective working directory
  is omitted, Driva mounts the current directory writable at its canonical
  same-path destination and uses it as the workspace. Without a configured
  rootfs, Bubblewrap builds a private root containing only conventional
  read-only system runtime paths in addition to that workspace; the host root
  and home are not selected implicitly.

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
      --config <CONFIG>        Configuration file (defaults to ./driva.toml when present)
      --template <NAME>        Apply a named execution template; may be repeated
      --read <MOUNT>           Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>          Add a writable mount as SOURCE or SOURCE:DESTINATION
      --no-write               Make every host mount read-only, overriding configuration and templates
      --path <DIRECTORY>       Add a host directory read-only and prepend it to the isolated PATH
      --backend <BACKEND>      Select the isolation backend
      --network                Permit networking (disabled otherwise)
      --no-network             Disable networking, overriding configuration and templates
  -i, --interactive            Allocate an interactive terminal
      --no-interactive         Disable interactivity, overriding a template
      --no-new-session         Keep the caller's terminal session instead of starting a new one
      --dry-run                Print the validated request and backend invocation without executing it
      --rootfs <DIRECTORY>     Override the Bubblewrap root filesystem
      --temporary <DIRECTORY>  Add an empty writable filesystem discarded after execution
      --workdir <WORKDIR>      Override the isolated working directory (defaults to a writable current-dir workspace)
      --inherit-env            Inherit environment variables from the host shell
      --env <ENVIRONMENT>      Set an environment variable as NAME=VALUE
      --command <COMMAND>      Override the template command or supply the executable
  -h, --help                   Print help
```

The trailing `<COMMAND>...` is the program and its arguments. Use `--` to
separate them from Driva's own flags when the command has flags of its own:

```sh
driva run --write . -- cargo test
driva run --template test --command cargo -- check
driva run --read ~/.cargo/registry --write . --network -- cargo update
driva run --workdir /workspace --write .:/workspace -- cargo build
driva run --backend bwrap --rootfs /srv/rootfs --temporary /home -- command
driva run --path ./tools -- project-tool
driva run --inherit-env -- command
driva run --env RUST_LOG=debug -- env
```

`--command <COMMAND>` replaces the template's command for this invocation. It
can also supply the executable without a template.

Policy options (shared by `run` and `shell`):

| Option | Effect |
| --- | --- |
| `--template <NAME>` | Apply a built-in or project-defined execution template; repeat it to combine templates in option order. |
| `--read <MOUNT>` | Bind-mount a host path read-only. Repeatable. |
| `--write <MOUNT>` | Bind-mount a host path read-write. Repeatable. |
| `--no-write` | Make every host bind mount read-only, overriding project configuration, templates, and `--write`. |
| `--path <DIRECTORY>` | Bind-mount a host directory read-only and prepend it to the isolated `PATH`. Repeatable. |
| `--backend <BACKEND>` | Select the isolation backend for this invocation (`bwrap`). |
| `--network` | Enable networking (otherwise the sandbox has none). |
| `--no-network` | Disable networking, overriding global configuration and templates. |
| `-i`, `--interactive` | Allocate an interactive terminal (stdin + TTY). |
| `--no-interactive` | Disable interactivity requested by a template. |
| `--no-new-session` | Run the command in the caller's terminal session instead of a new one. Bubblewrap otherwise passes `--new-session`, which detaches the controlling terminal to block TIOCSTI input injection; disable it only for tools that require the inherited session. |
| `--dry-run` | Print the validated request and the exact backend invocation without executing anything. |
| `--rootfs <DIRECTORY>` | Override the prepared root filesystem for Bubblewrap. |
| `--temporary <DIRECTORY>` | Add an empty writable filesystem discarded after execution. Repeatable. |
| `--workdir <WORKDIR>` | Override the isolated working directory (must be absolute; omission defaults to a writable current-directory workspace). |
| `--inherit-env` | Inherit all environment variables from the host shell. |
| `--env NAME=VALUE` | Set an environment variable inside the container. Repeatable. |

Each `--path` directory must exist on the host. Relative paths are resolved
from the current host directory, and `~` is expanded from the host home
directory. Driva mounts each one read-only at its canonical host path inside
the isolation and prepends that path to `PATH` in option order. Preserving the
path allows tool managers such as Rustup to find state installed next to their
executable proxies. If configuration, a template, or `--env` supplies `PATH`,
the additions are prepended to that value; otherwise Driva retains its
conventional system path.

Template settings overlay the global configuration, and one-off CLI values
overlay the templates. `--template` may be repeated. Templates are combined
in option order: mounts and PATH additions accumulate, while later templates
replace earlier scalar settings, environment values with the same name, and
commands. A later template that does not set a scalar or command leaves the
earlier value intact. Scalar values such as backend, rootfs, and working
directory use CLI, then later templates, then earlier templates, then
configuration precedence. Mounts and PATH additions accumulate in layer order.
`--command` replaces the entire combined template command (including its
initial arguments), after which arguments
following `--` are appended. Explicit `network = false` in a template
overrides enabled project networking, while `--network` and
`--no-network` provide the final CLI choice. `--no-interactive` similarly
overrides an interactive template. `--no-write` is applied after all mounts
are combined, making every host bind mount read-only regardless of its source;
temporary filesystems are unaffected because
they cannot modify mounted host data. Without a template or `--command`, at
least one positional command argument remains required at runtime.

`--inherit-env` uses the host process environment as the base environment for
the session. Project configuration, template environment values, and `--env`
then override inherited variables in that order. Without this option, the host
environment remains isolated except for the documented `HOME` and `TERM`
defaults below.

When a template is selected and the effective configuration does not set
`HOME`, Driva inherits `HOME` from the host. An explicit project, template, or
`--env` value overrides that inherited default.

Bubblewrap also inherits `TERM` from the host when the effective configuration
does not set it. An explicit project, template, or `--env` value takes
precedence.

### Execution templates

List effective built-in and project-defined templates with:

```console
$ driva templates
claude	Run Claude Code interactively against the current project
claude-exec	Run Claude Code non-interactively against the current project
codex	Run the host's Codex binary interactively in Bubblewrap
codex-exec	Run OpenAI Codex non-interactively against the current project
codex-runtime	Run OpenAI Codex interactively against the current project
sbt	Scala sbt
```

The built-in `codex-runtime` template runs a pinned, prepared Codex
installation interactively. Run `driva runtime install codex@latest` or select
an exact version before using it. Installation uses Podman and
`node:22-bookworm` once to build a complete filesystem under
`~/.local/share/driva/runtimes/codex/VERSION`; normal executions expose the
active version read-only through Bubblewrap and do not use Podman.

The template uses a shell wrapper to resolve the isolated working directory
with `pwd -P`, then passes an ephemeral Codex setting that marks that exact
path as trusted, avoiding the directory trust prompt before
project-scoped configuration is loaded. It disables Codex's inner sandbox and
relies on Driva's outer Bubblewrap isolation. It mounts the current directory
writable at `/driva`, enables networking, and puts `/root/.codex` on a
private writable temporary filesystem for disposable Codex state. It then mounts
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
replace the invoking user's home directory with a private temporary filesystem. Only the
current project and `~/.codex` are mounted back into the sandbox. The project
is mounted at its canonical host path, while Codex state is mounted at
`/root/.codex`. A shell wrapper supplies that path to Codex's project-trust
override and forwards template arguments unchanged.
The local executable must therefore be available on the standard system `PATH`
outside the user's home directory. Like the prepared template, they trust the
isolated workspace and disable Codex's inner sandbox.

The built-in `claude` template runs the host's `claude` executable
interactively; `claude-exec` adds `--print` for non-interactive use. Both use
Bubblewrap, put `~/.local/bin` on the isolated `PATH` so the host `claude`
binary is found, mount the current project as a writable workspace, and enable
networking. They mount `~/.local/share` read-only and `~/.claude` and
`~/.claude.json` writable so a host Claude Code login and its session state
persist. [Anthropic's authentication
documentation](https://docs.anthropic.com/en/docs/claude-code/iam) describes
the Linux credential store. A host Claude Code login must exist before using
these templates.

```sh
driva run --template codex
driva run --template codex --template project-policy
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

The installer uses the `podman` executable to create a temporary
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
      --config <CONFIG>        Configuration file (defaults to ./driva.toml when present)
      --template <NAME>        Apply a named execution template; may be repeated
      --read <MOUNT>           Add a read-only mount as SOURCE or SOURCE:DESTINATION
      --write <MOUNT>          Add a writable mount as SOURCE or SOURCE:DESTINATION
      --no-write               Make every host mount read-only, overriding configuration and templates
      --path <DIRECTORY>       Add a host directory read-only and prepend it to the isolated PATH
      --backend <BACKEND>      Select the isolation backend
      --network                Permit networking (disabled otherwise)
      --no-network             Disable networking, overriding configuration and templates
  -i, --interactive            Allocate an interactive terminal
      --no-interactive         Disable interactivity, overriding a template
      --no-new-session         Keep the caller's terminal session instead of starting a new one
      --dry-run                Print the validated request and backend invocation without executing it
      --rootfs <DIRECTORY>     Override the Bubblewrap root filesystem
      --temporary <DIRECTORY>  Add an empty writable filesystem discarded after execution
      --workdir <WORKDIR>      Override the isolated working directory (defaults to a writable current-dir workspace)
      --inherit-env            Inherit environment variables from the host shell
      --env <ENVIRONMENT>      Set an environment variable as NAME=VALUE
  -h, --help                   Print help
```

```sh
driva shell --write .
```

## Configuration file (`driva.toml`)

Loaded from `--config <FILE>`, or `./driva.toml` when present. Defaults are
shown below; Bubblewrap's `rootfs` is optional.

```toml
[isolation]
backend = "bwrap"

[isolation.bwrap]
# rootfs = "/var/lib/driva/rootfs/busybox" # optional prepared userspace
# workdir = "/workspace"         # optional
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
backend = "bwrap"                # optional; only "bwrap" is supported
rootfs = "/srv/driva/rootfs/rust" # optional; Bubblewrap only
workdir = "/workspace"           # optional
path = ["~/.cargo/bin"]          # optional; read-only PATH additions
network = false
interactive = false

[[template.lint.workspace-mount]] # optional; at most one per template
source = "."
destination = "/workspace"      # optional; defaults to canonical source
access = "write"

[[template.lint.mount]]
kind = "temporary"
destination = "/root/.cache"

[template.lint.environment]
RUST_LOG = "info"
```

Template fields are optional except that the effective command must be
non-empty. Unknown fields are rejected. `command` is an array of the executable
and its initial arguments. `backend`, `rootfs`, `workdir`,
`path`, networking, interactivity, environment, and mounts correspond to the
same per-run CLI concepts. `rootfs` overrides `[isolation.bwrap].rootfs` when
the template selects Bubblewrap, while `--rootfs` overrides both; `~` is
expanded using `$HOME` and the tree is mounted read-only.
When neither is set, Driva constructs a private root with conventional host
system runtime paths mounted read-only. It does not expose the host root, home,
or other data paths beyond the default current-directory workspace.
Mounts default to `kind = "bind"`; these require a host `source`, accept an
optional `destination` and `access`, and default to read-only. A
`kind = "temporary"` mount requires only an absolute `destination`; it creates
an empty writable filesystem that is discarded after execution. Temporary
mounts are implemented with native tmpfs mounts by Bubblewrap. A leading `~`
in a temporary destination is expanded using the host
`$HOME`. Repeatable `--temporary` values create the same mount kind.
Bind mount destinations are optional in configuration. When omitted, the
canonicalized host source is also used as the isolated destination. A host
bind can be placed beneath a temporary mount, allowing selected files to
persist inside otherwise-disposable state. `workspace-mount` is bind-only and
uses its resolved destination as the working directory. A template can contain
at most one such entry; it supersedes `workdir` when
both are set. Application-specific behavior, such as Codex project trust,
belongs in the template command. Project templates appear in `driva templates`;
project definitions replace built-ins with the same name.

If `--workdir`, a template workdir or workspace mount, and the selected
backend's configured `workdir` are all omitted, Driva creates an implicit
writable workspace mount from `.` to its canonical same-path destination and
uses that destination as the working directory. An explicit mount at the same
destination replaces this default mount, so `--read .` can make the default
workspace read-only.

Bubblewrap can use `rootfs` as a prepared filesystem tree rather than pulling
an OCI image. The tree must contain `/proc`, `/dev`, `/tmp`, each configured
temporary mount point, and any working directory or bind destination that is
not created beneath a temporary mount. Driva exposes it read-only. With or
without a prepared tree, Driva creates private `/proc` and `/dev` mounts and a
writable tmpfs at `/tmp`, and clears the inherited host environment.

## Further reading

- [`README.md`](../README.md) — overview and quick start.
- [`DESIGN.md`](../DESIGN.md) — architecture and isolation model.
