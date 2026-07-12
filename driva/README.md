# Driva

Driva runs a command in a disposable Docker container with no host access and
no network access unless they are explicitly granted.

```sh
cargo run -- run --write . -- cargo test
cargo run -- run --read ~/.cargo/registry --write . --network -- cargo update
cargo run -- shell --write .
```

Mount arguments accept `SOURCE` or `SOURCE:DESTINATION`. A relative source with
no destination is placed below the configured container working directory;
`.` is mounted at the working directory. Mount sources must exist. Use
`--dry-run` to inspect the effective grants and Docker invocation.

Projects can provide `driva.toml`:

```toml
[isolation]
backend = "docker"

[isolation.docker]
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
