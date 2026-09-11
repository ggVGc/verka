# Styra in Lua

What it takes to talk to `styra-server` from a Lua script, which is less than
it looks: the wire vocabulary is `require`d from where it is generated, so what
is actually here is a socket, a codec, and an example.

| | |
|---|---|
| `styra/client.lua` | The JSON codec and the Unix socket the protocol leaves to its callers. |
| `styra/json.lua` | A toy codec, so a script runs before anything is installed. |
| `examples/styra-ask.lua` | A small client built on both halves. |
| `../styra-protocol/lua/styra/protocol.lua` | **The vocabulary**, generated, living where it is generated from. |

## The vocabulary, which is not here

`styra.protocol` is generated from the Serde type definitions in Rust, and it
lives beside them — in `../styra-protocol/lua`. That is the whole point of
generating it: a copy checked in here would be a second home for the protocol,
and a second home is where drift starts. Regenerate it with

```sh
cargo run -p styra-protocol --bin styra-codegen -- lua        # rewrite it
cargo run -p styra-protocol --bin styra-codegen -- lua PATH   # write it elsewhere
```

and a test in the crate fails the moment the checked-in file stops matching the
definitions. See `../styra-protocol/lua/README.md`.

Lua has no manifest to declare a dependency in, so the two directories are
joined by a search path. The example sets it itself, relative to where it is
run from; a project of your own would set it once:

```sh
export LUA_PATH="/path/to/styra-protocol/lua/?.lua;/path/to/styra-lua/?.lua;;"
```

The protocol owns no transport and no JSON codec, exactly as the Rust crate
owns neither. A request is a plain table; carrying it is somebody's business,
and the next section is this repository's answer to whose.

- `protocol.request.<operation>(data)` builds a request, raising if the data
  would be refused: a missing field, a misspelled one, a value of the wrong
  shape. `Request` denies unknown fields, so an unchecked client's typo comes
  back as a rejected round trip far from the line that caused it.
- `protocol.build(operation, data)` does the same for an operation named at
  runtime, and `protocol.OPERATIONS` lists them all.
- `protocol.validate(type_name, value)` is the same check without the raise,
  returning `false` and a message naming the field that was wrong. It works on
  anything in `protocol.types` — validating an `InteractionUpdate` off the
  update stream as readily as a request.
- `protocol.unwrap(reply)` returns the response table or `nil` and the server's
  error; `protocol.expect(reply, kind)` also checks the response is the one the
  request asked for and returns its data.
- `protocol.<Enum>.<VARIANT>` are the wire spellings (`protocol.Contract.FILES`,
  `protocol.Response.ANSWER`), and `protocol.enums.<Enum>` lists each in
  declaration order.
- `protocol.types` describes every type: `kind`, `fields` with `required`, and
  each enum's tagging — enough to drive a form, a picker, or a decoder.
- `protocol.null` is the value meaning JSON null, for the fields where saying
  null means something (clearing a Session name). Lua cannot tell an absent key
  from one set to `nil`, so pass `protocol.null` explicitly; point it at your
  encoder's own sentinel with `protocol.use_null` first.

## The hand-written half

`styra/client.lua` is the JSON codec and the socket, written once here instead
of again in every script. It sits on top of `styra.protocol` exactly as a
script would, and nothing in it is generated or privileged.

```lua
local client = require("styra.client")
local protocol = require("styra.protocol")

local styra = assert(client.open())           -- or { socket = "/path/to/styra.sock" }
local health = assert(styra:call(protocol.request.health(), protocol.Response.HEALTH))
print(health.service .. " is up")
```

- `client.open(options)` returns a client. `options.socket` defaults to
  `$XDG_RUNTIME_DIR/styra/styra.sock`, the socket `styractl` uses. It also
  wires the codec's null into `protocol.use_null` for you.
- `styra:call(request, expected)` sends one request and returns the data of the
  response it must answer with, or `nil` and a message. `styra:unwrap(request)`
  returns whatever response came back, for callers that handle more than one.
- `client.given(value)` turns both `nil` and the null sentinel into `nil`, which
  is what reading a nullable field for display wants.
- `options.json` and `options.exchange` replace the codec and the transport. A
  script running inside an editor, or a test, has its own way to move bytes;
  the protocol never sees the difference.

Needs LuaSocket with Unix-socket support or `socat` on `PATH`, and uses
lua-cjson or dkjson if either is installed, falling back to `styra/json.lua`.

## The example

`examples/styra-ask.lua` is a small working client — health, Workspaces, live
interactions, and a typed question put to a session:

```sh
lua examples/styra-ask.lua health
lua examples/styra-ask.lua interactions
lua examples/styra-ask.lua ask styra-1 'which files handle auth?'
```

`ask` is the interesting one: it sends the question under the `lines` contract,
polls `updates` until the turn completes, and asks `turn_answer` for the parsed
result — the three requests the protocol's own shape calls for, since a
connection carries one request and a turn takes minutes.

Nothing in it restates the wire format, and nothing in it opens a socket
either: it is only the commands, which is what an example should be left as
once the two halves above exist.

See `../styra-protocol/src/protocol/README.md` for what the operations mean and
how the transport behaves, and `../styra-elixir/` for the same three pieces in
Elixir.
