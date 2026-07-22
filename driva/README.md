# Driva

Driva runs a command in a disposable isolated environment with no host data
access and no network access unless they are explicitly granted. Bubblewrap is
the default backend. Without configuration, Driva constructs a private root
that contains only the host's read-only system runtime, so `/bin/sh` and normal
OS tools are available without exposing the host root, home, or current
directory.

```sh
cargo run -- run --write . -- cargo test
cargo run -- run --read ~/.cargo/registry --write . --network -- cargo update
cargo run -- run --path ./tools -- project-tool
cargo run -- run --backend bwrap --rootfs /srv/rootfs --temporary /home -- command
cargo run -- shell --write .
```

Named templates bundle a command, backend, image or rootfs, mounts, and
policy. Driva ships interactive and non-interactive templates for Codex and
Claude Code:

```sh
cargo run -- templates
cargo run -- run --template codex
cargo run -- runtime install codex@latest
cargo run -- run --template codex-runtime
cargo run -- run --template codex-exec -- "fix the failing tests"
cargo run -- run --template codex --command /bin/sh
cargo run -- run --template codex --no-network
cargo run -- run --template codex --no-write
cargo run -- run --template claude
cargo run -- run --template claude-exec -- "fix the failing tests"
```

The `codex` template runs the host's Codex binary with Bubblewrap and mounts
the current project writable at its canonical host path. A small shell wrapper
passes that working directory to Codex as a trusted project. The template makes
the host's `~/.codex` directory available at `/root/.codex`.

The `codex-exec` template is the non-interactive form of `codex`; it uses the
same host executable and same-path workspace mount. `codex-runtime`
instead uses a pinned runtime prepared by `driva runtime install` and exposed
read-only through Bubblewrap. Installation uses Podman once to install Node and
Codex into a complete versioned filesystem under
`~/.local/share/driva/runtimes`; normal executions do not use Podman. The
runtime template mounts the project at `/driva`, uses disposable Codex state,
and mounts the host `~/.codex/auth.json` at
`/root/.codex/auth.json`.

The Claude Code templates continue to use Podman with Node 22, mount the
current project at its canonical host path, and mount only
the Linux credential file `~/.claude/.credentials.json` at
`/root/.claude/.credentials.json`; all other Claude state is disposable. They
require a host login created by Claude Code on Linux.

The full command-line reference — every subcommand, option, the mount
grammar, and the `driva.toml` schema — lives in [`docs/cli.md`](docs/cli.md).
Its help-text blocks are checked against the compiled binary by
`tests/cli_docs.rs`; regenerate them with
`DRIVA_UPDATE_DOCS=1 cargo test --test cli_docs` after changing the CLI.

Mount arguments accept `SOURCE` or `SOURCE:DESTINATION`. A relative source with
no destination is placed below the configured container working directory;
`.` is mounted at the working directory. Mount sources must exist. Use
`--dry-run` to inspect the effective grants and backend invocation.

Use repeatable `--path DIRECTORY` options to make host tool directories
available without granting write access. Driva mounts each directory read-only
at its canonical host path inside the isolation and prepends those paths to
`PATH` in the order given. This works with Bubblewrap, Podman, and Docker while
preserving paths used by tool managers such as Rustup.

Launch settings use the same vocabulary in templates and on the command line.
For example, `--command`, `--backend`, `--image`, `--rootfs`, repeatable
`--temporary`, `--workdir`, `--path`, networking, interactivity, environment, and
mounts all override or extend the corresponding project/template settings.
Scalar precedence is CLI, then template, then project configuration. When
`--command` is given, it replaces the template's executable and initial
arguments; trailing command arguments are appended to the replacement.
`--no-write` is a final safety override: it turns every host bind mount,
including mounts from project configuration, templates, and `--write`, into a
read-only mount. Temporary filesystems remain
available because they cannot modify the mounted host data.

Projects can provide `driva.toml`:

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
kind = "temporary"
destination = "/workspace/.cache"

[network]
enabled = false

[template.test]
description = "Run this project's tests"
command = ["cargo", "test"]
backend = "podman"
image = "rust:1.88"

[[template.test.workspace-mount]]
source = "."
destination = "/workspace"
access = "write"

# A workspace mount also sets the isolated working directory. When
# destination is omitted, the canonical source path is used inside too.
```

The library exposes the backend-independent `ExecutionRequest` and `Isolation`
interface. `validate_request` resolves host sources and rejects invalid or
conflicting grants; `execute` validates before dispatching to a backend.

Bubblewrap is the default for lightweight Linux execution. Its
configuration-free mode exposes conventional system runtime paths read-only
inside an otherwise private root. A prepared root filesystem can be selected
when commands need a different userspace:

```toml
[isolation]
backend = "bwrap"

[isolation.bwrap]
rootfs = "/var/lib/driva/rootfs/busybox"
workdir = "/tmp"
```

When configured, the rootfs must contain `/proc`, `/dev`, `/tmp`, each
temporary mount point, and every working directory or bind destination not
created beneath a temporary mount. Driva exposes it read-only and places a
private writable tmpfs at `/tmp`. Bubblewrap uses `--rootfs` instead of an OCI
`--image`.

Podman and Docker remain available by setting `isolation.backend` to
`"podman"` or `"docker"`. Their default image is the minimal
`docker.io/library/busybox:latest`.

Driva keeps every command attached to its caller and returns its exit status.
For a detachable interactive command, run Driva under a terminal multiplexer:

```sh
tmux new-session -s codex -- driva run --template codex
```

tmux or screen then owns terminal persistence and reattachment; Driva continues
to own isolation policy and cleanup.
