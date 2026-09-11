# Styra in Elixir

What it takes to talk to `styra-server` from Elixir, which is less than it
looks: the wire vocabulary is depended on rather than written, so what is
actually here is a socket, a codec, and an example.

| | |
|---|---|
| `lib/styra/client.ex` | The JSON codec and the Unix socket the protocol leaves to its callers. |
| `examples/styra_ask.exs` | A small client built on both halves. |
| `{:styra_protocol, path: "../styra-protocol/elixir"}` | **The vocabulary**, generated, living where it is generated from. |

No dependency needs installing for the client itself: Elixir has carried a
`JSON` module since 1.18, and OTP speaks Unix sockets natively. `ex_doc` is
dev-only, for `mix docs`.

## The vocabulary, which is not here

`Styra.Protocol` is generated from the Serde type definitions in Rust, and it
lives beside them — in `../styra-protocol/elixir`, as a package this one
depends on. That is the whole point of generating it: a copy checked in here
would be a second home for the protocol, and a second home is where drift
starts. Regenerate it with

```sh
cargo run -p styra-protocol --bin styra-codegen -- elixir
```

and a test in the crate fails the moment the checked-in package stops matching
the definitions. See `../styra-protocol/elixir/README.md`.

It owns no transport and no JSON codec, exactly as the Rust crate owns neither.
A request is a plain map; carrying it is somebody's business, and the next
section is this repository's answer to whose.

- `Styra.Protocol.Request.<operation>(data)` builds a request, returning
  `{:ok, request}` or `{:error, message}`: a missing field, a misspelled one, a
  value of the wrong shape. `Request` denies unknown fields, so an unchecked
  client's typo comes back as a rejected round trip far from the line that
  caused it. Each has a `!` form that returns the request and raises.
- `Styra.Protocol.build/2` does the same for an operation named at runtime, and
  `operations/0` lists them all.
- `Styra.Protocol.validate/2` is the same check on any wire type — validating
  an `InteractionUpdate` off the update stream as readily as a request.
- `Styra.Protocol.unwrap/1` returns `{:ok, response}` or the server's error;
  `expect/2` also checks the response is the one the request asked for and
  returns its data.
- `Styra.Protocol.<Enum>` is a module per enum: `Contract.files()` is the wire
  spelling, `Contract.parse("files")` reads one back as `{:ok, :files}`, and
  `values/0` lists them in declaration order. The operations enum is the
  exception — that name belongs to the constructors, so the list is
  `Styra.Protocol.operations/0`.
- `Styra.Protocol.types/0` describes every type: `:kind`, `:fields` with
  `:required`, and each enum's tagging — enough to drive a form, a picker, or a
  decoder.

Requests come out with string keys, which is what an encoder wants. The data
you pass in may use atom keys, which is what writing Elixir by hand wants. A
nullable-but-required field needs no sentinel, unlike the Lua library: a key
set to `nil` is present, and a key left out is not.

Every `@doc` in the generated module is the doc comment the protocol author
wrote in Rust, field tables and all. `mix docs` in `../styra-protocol/elixir`
renders the protocol's own explanation of each of its forty-odd operations;
`mix docs` here renders this client, since each package documents what it owns.

## The hand-written half

`Styra.Client` is the JSON codec and the socket, written once here instead of
again in every script. It sits on top of `Styra.Protocol` exactly as a caller
would, and nothing in it is generated or privileged.

```elixir
alias Styra.Client
alias Styra.Protocol.{Request, Response}

{:ok, styra} = Client.new()            # or Client.new(socket: "/path/to/styra.sock")
{:ok, health} = Client.call(styra, Request.health(), Response.health())
IO.puts("#{health["service"]} is up")
```

- `Client.new/1` returns a client. `:socket` defaults to
  `$XDG_RUNTIME_DIR/styra/styra.sock`, the socket `styractl` uses. Nothing is
  opened: a connection carries one request and one response, so each exchange
  is a whole connection.
- `Client.call(styra, request, expected)` sends one request and returns the
  data of the response it must answer with. `Client.unwrap/2` returns whatever
  response came back, and `Client.request/2` the raw decoded reply.
- Constructors return `{:ok, request}`, and every one of these takes that
  tuple as readily as the map inside it, so the two read the same at a call
  site.
- `:json` and `:exchange` replace the codec and the transport. A script running
  inside a release, or a test, has its own way to move bytes; the protocol
  never sees the difference.

## The example

`examples/styra_ask.exs` is a small working client — health, Workspaces, live
interactions, and a typed question put to a session:

```sh
mix run examples/styra_ask.exs health
mix run examples/styra_ask.exs interactions
mix run examples/styra_ask.exs ask styra-1 which files handle auth?
```

`ask` is the interesting one: it sends the question under the `lines` contract,
polls `updates` until the turn completes, and asks `turn_answer` for the parsed
result — the three requests the protocol's own shape calls for, since a
connection carries one request and a turn takes minutes.

Nothing in it restates the wire format, and nothing in it opens a socket
either: it is only the commands, which is what an example should be left as
once the two halves above exist.

See `../styra-protocol/src/protocol/README.md` for what the operations mean and
how the transport behaves, and `../styra-lua/` for the same three pieces in
Lua.
