# Driva

Driva runs a command in a disposable isolated environment with no host access
and no network access unless they are explicitly granted.
Bubblewrap is the default backend. Configure a prepared root filesystem before
the first run; Driva deliberately does not default it to the host root.

```sh
cargo run -- run --write . -- cargo test
cargo run -- run --read ~/.cargo/registry --write . --network -- cargo update
cargo run -- shell --write .
```

Named templates bundle a command, backend, image, mounts, and policy. Driva
ships interactive and non-interactive Codex templates:

```sh
cargo run -- templates
cargo run -- run --template codex
cargo run -- run --template codex-exec -- "fix the failing tests"
cargo run -- run --template codex --no-network
```

These templates use Podman with Node 22, mount the current project writable at
`/workspace`, enable networking, and mount only `~/.codex/auth.json` writable
at `/root/.codex/auth.json` so Codex can use and refresh an existing file-backed
login. All other Codex state lives in the disposable container. The auth file
contains access tokens, so select the template only for code you trust. Hosts
using an OS keyring must create a file-backed Codex login before using the
built-in template, or replace it with a project template using another
authentication scheme.

The full command-line reference — every subcommand, option, the mount
grammar, and the `driva.toml` schema — lives in [`docs/cli.md`](docs/cli.md).
Its help-text blocks are checked against the compiled binary by
`tests/cli_docs.rs`; regenerate them with
`DRIVA_UPDATE_DOCS=1 cargo test --test cli_docs` after changing the CLI.

Mount arguments accept `SOURCE` or `SOURCE:DESTINATION`. A relative source with
no destination is placed below the configured container working directory;
`.` is mounted at the working directory. Mount sources must exist. Use
`--dry-run` to inspect the effective grants and backend invocation.

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
workdir = "/workspace"

[[template.test.mount]]
source = "."
destination = "/workspace"
access = "write"
```

The library exposes the backend-independent `ExecutionRequest` and `Isolation`
interface. `validate_request` resolves host sources and rejects invalid or
conflicting grants; `execute` validates before dispatching to a backend.

Bubblewrap is the default for lightweight synchronous Linux execution using a
prepared root filesystem:

```toml
[isolation]
backend = "bwrap"

[isolation.bwrap]
rootfs = "/var/lib/driva/rootfs/busybox"
workdir = "/tmp"
```

The rootfs must contain `/proc`, `/dev`, `/tmp`, the working directory, and
every configured mount destination. Driva exposes the rootfs read-only and
places a private writable tmpfs at `/tmp`. Bubblewrap does not currently
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
