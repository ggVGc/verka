# Driva

Driva runs a command in a disposable Podman container with no host access and
no network access unless they are explicitly granted.
The default image is the minimal `docker.io/library/busybox:latest`; configure
a tool-specific image when the command needs more than BusyBox provides. The
zero-configuration working directory is `/tmp`, which exists in BusyBox.

```sh
cargo run -- run --write . -- cargo test
cargo run -- run --read ~/.cargo/registry --write . --network -- cargo update
cargo run -- shell --write .
```

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
backend = "podman"

[isolation.podman]
image = "rust:1.88"
workdir = "/workspace"

[[mount]]
source = "."
destination = "/workspace"
access = "write"

[network]
enabled = false
```

The library exposes the backend-independent `ExecutionRequest` and `Isolation`
interface. `validate_request` resolves host sources and rejects invalid or
conflicting grants; `execute` validates before dispatching to a backend.

Podman is the default backend. Docker remains available by setting
`isolation.backend = "docker"` and configuring `[isolation.docker]`.

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
