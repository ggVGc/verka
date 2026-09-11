# The Styra protocol in Lua

`styra/protocol.lua` is the Styra wire vocabulary as a Lua module: every
operation, field name, and enum spelling, read out of the Serde type
definitions in the Rust crate above this directory by its generator
(`../src/codegen`).

It is generated. Do not edit it by hand — regenerate it:

```sh
cargo run -p styra-protocol --bin styra-codegen -- lua
```

A test in the crate fails the moment the checked-in file stops matching the
definitions, so the copy here cannot describe a protocol the server does not
speak. It is checked in rather than generated on demand so that a Lua project
can use it without a Rust toolchain.

It lives here, beside the definitions it is generated from, rather than in
`styra-lua`, so that the protocol has exactly one home. Lua has no manifest to
declare a dependency in, so what points at it is a search path:

```sh
export LUA_PATH="/path/to/styra-protocol/lua/?.lua;/path/to/styra-lua/?.lua;;"
```

`../../styra-lua` adds the two things this deliberately does not have — a JSON
codec and a socket — and its example sets that path itself, relative to where
it is run from. See `../../styra-lua/README.md` for how to actually talk to a
server, and `../src/protocol/README.md` for what the operations mean.
