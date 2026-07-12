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
