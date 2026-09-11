# The Styra protocol in Lua

`styra/protocol.lua` is a generated Lua module carrying the same wire
vocabulary `styra-protocol` defines in Rust: every operation, every field name,
every enum spelling, read out of the Serde type definitions themselves by
`styra-protocol`'s Lua generator (`src/lua`).

It is checked in so a Lua client can simply vendor or `require` it, and a test
in the crate fails the moment it stops matching the definitions. Do not edit it
by hand — regenerate it:

```sh
cargo run -p styra-protocol --bin styra-protocol-lua          # rewrite this copy
cargo run -p styra-protocol --bin styra-protocol-lua -- PATH  # write it elsewhere
```

## Using it

The module owns no transport and no JSON codec, exactly as the Rust crate owns
neither. A request is a plain table; carrying it is your business — one JSON
object per line over the server's Unix socket.

```lua
local protocol = require("styra.protocol")
protocol.use_null(json.null)  -- whatever your encoder spells null as

local line = json.encode(protocol.request.send_message({
  id = session,
  message = { text = "which files handle auth?", contract = protocol.Contract.FILES },
}))
-- …write `line .. "\n"`, read one line back…
local answer, err = protocol.expect(json.decode(reply), protocol.Response.ANSWER)
```

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

See `../src/protocol/README.md` for what the operations mean and how the
transport behaves.
