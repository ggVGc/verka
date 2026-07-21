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
cargo run -- run --backend bwrap --rootfs /srv/rootfs --tmpfs /home -- command
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
cargo run -- run --template codex --no-network
cargo run -- run --template claude
cargo run -- run --template claude-exec -- "fix the failing tests"
```

The `codex` template runs the host's Codex binary with Bubblewrap and mounts
the current project writable below `/tmp/driva` at its canonical host path
(for example, `/tmp/driva/home/me/project`). It makes the host's `~/.codex`
directory available at `/root/.codex`.

The `codex-runtime` and `codex-exec` templates instead use a pinned runtime
prepared by `driva runtime install` and expose it read-only through Bubblewrap.
Installation uses Podman once to install Node and Codex into a complete
versioned filesystem under `~/.local/share/driva/runtimes`; normal Codex runs
do not use Podman. These templates mount the project below `/driva` at its
canonical host path, use disposable Codex state, and mount the host
`~/.codex/auth.json` at `/root/.codex/auth.json`.

The Claude Code templates continue to use Podman with Node 22, mount the
current project below `/driva` at its canonical host path, and mount only
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
For example, `--backend`, `--image`, `--rootfs`, repeatable `--tmpfs`,
`--workdir`, `--path`, networking, interactivity, environment, and mounts all
override or extend the corresponding project/template settings. Scalar
precedence is CLI, then template, then project configuration.

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

[network]
enabled = false

[template.test]
description = "Run this project's tests"
command = ["cargo", "test"]
backend = "podman"
image = "rust:1.88"
workspace_root = "/workspace"

# Driva mounts the project at
# /workspace/<canonical host path> and uses it as the working directory.
```

The library exposes the backend-independent `ExecutionRequest` and `Isolation`
interface. `validate_request` resolves host sources and rejects invalid or
conflicting grants; `execute` validates before dispatching to a backend.

Bubblewrap is the default for lightweight synchronous Linux execution. Its
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

When configured, the rootfs must contain `/proc`, `/dev`, `/tmp`, the working
directory, and every configured mount destination. Driva exposes it read-only
and places a private writable tmpfs at `/tmp`. Bubblewrap does not currently
support Driva's durable session commands or `--image`; use Podman or Docker
when those capabilities are required.

Podman and Docker remain available by setting `isolation.backend` to
`"podman"` or `"docker"`. Their default image is the minimal
`docker.io/library/busybox:latest`.

## Durable sessions

Long-running commands can be detached and managed later. Both Podman and
Docker sessions are labelled with their Driva session id so recovery can
rediscover the backend resource:

```sh
id=$(cargo run -- start --write . -- make watch)
cargo run -- inspect "$id"
cargo run -- attach "$id"
cargo run -- wait "$id"
cargo run -- terminate "$id" --grace 10
cargo run -- remove "$id"
cargo run -- list
cargo run -- recover
```

Session records and append-only observations are stored below
`$DRIVA_STATE_DIR`, `$XDG_STATE_HOME/driva`, or `~/.local/state/driva` (in
that order). Records retain the effective grant and environment variable
names, but deliberately omit environment values. Current process state is
always queried from the container backend; a missing resource is never
reported as a successful exit.

The production-hardening roadmap, lifecycle invariants, storage protocol, and
phase acceptance criteria are specified in
[`STAGE2_IMPLEMENTATION.md`](STAGE2_IMPLEMENTATION.md).
