# styra_protocol

The Styra wire vocabulary as an Elixir package: every operation, field name,
and enum spelling, read out of the Serde type definitions in the Rust crate
above this directory by its generator (`../src/codegen`).

`lib/styra/protocol.ex` is generated. Do not edit it by hand — regenerate it:

```sh
cargo run -p styra-protocol --bin styra-codegen -- elixir
```

A test in the crate fails the moment the checked-in file stops matching the
definitions, so the copy here cannot describe a protocol the server does not
speak. It is checked in rather than generated at build time so that an Elixir
project can depend on it without a Rust toolchain.

It lives here, beside the definitions it is generated from, rather than in
`styra-elixir`, so that the protocol has exactly one home. `styra-elixir`
depends on this package:

```elixir
{:styra_protocol, path: "../styra-protocol/elixir"}
```

and adds the two things this deliberately does not have — a JSON codec and a
socket. See `../../styra-elixir/README.md` for how to actually talk to a
server, and `../src/protocol/README.md` for what the operations mean.

`mix docs` renders the protocol's own explanation of each of its forty-odd
operations, which is the same prose the Rust definitions carry.
