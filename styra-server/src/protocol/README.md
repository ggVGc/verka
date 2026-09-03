# Styra client/server protocol

This directory is the source of truth for the protocol between the `styra`
client and `styra-server`.

## Transport

- The transport is a Unix domain socket.
- A connection carries exactly one request followed by exactly one response.
- Each message is UTF-8 JSON terminated by a newline.
- Requests are limited to 8 MiB.

`Request` is tagged by `operation` and carries variant fields under `data`.
`Response` is tagged by `type` and carries variant fields under `data`. Every
response is wrapped in `WireResponse`, tagged by `status` as either `ok` or
`error`.

For example:

```json
{"operation":"health"}
```

```json
{"status":"ok","response":{"type":"health","data":{"service":"styra"}}}
```

## Update cursors

The `updates` request supplies the last observed sequence in `after`. Its
response returns updates after that sequence and a `next` cursor. Passing that
`next` value as the following `after` value is safe. Multiple clients may hold
independent cursors.

Both `updates` and `stored_session` carry a `raw` flag, defaulting to `true`.
Setting it to `false` omits the verbatim agent wire lines: `updates` filters out
`raw` updates, and `stored_session` returns an empty `raw` list without
re-reading the journal to rebuild it. A client that renders only decoded events
should pass `false` — the wire lines dominate the size of a long interaction.

`recent_updates` returns at most the newest requested number of conversation
events: operator messages, agent messages, errors, and model changes. Tool,
lifecycle, diagnostic, and raw-wire updates do not consume the limit. Its
`next` cursor points to the true end of the complete stream, so a client can
show a short tail and continue with ordinary incremental `updates` polling
without later receiving the omitted prefix.

`interaction_snapshot` is the self-contained view-loading operation. Its
`preview` scope returns that bounded conversation tail together with the
interaction's current summary and durable input queue; its `full` scope returns
the complete update stream. This lets a UI send one request while navigating
and turn the eventual response into a locally tagged event without performing
more round trips for small lifecycle state.

A resumed interaction seeds its update stream from stored history. The resume
response's `updates_after` cursor lets the resuming client skip history it has
already rendered; a newly attaching client starts from zero.

## Typed turn answers

A turn may name a `contract` — `text`, `lines`, `files`, or `json` — on its
`send_message` message, or on the seed `message` of a `create_session`. The
server frames that message with instructions describing the shape and the
`<styra:answer>` … `</styra:answer>` delimiters the reply must sit in, and
records the contract with the session. Framing is server-side so every client
asks for a shape in the same words; a client that framed its own message would
be sending an answer the parser has never been taught to read.

Because the framing is appended server-side, the message a client sees echoed
back in the event stream is the framed one. `contract::unframe` is the exact
inverse of the framing and recovers the operator's own text and the contract
from it, so a client can show what they wrote rather than the boilerplate; the
verbatim line is still in the journal and the raw stream either way.

The answer is fetched separately, not returned by `send_message`: a turn takes
minutes, and a connection carries one request. The client polls `updates` as it
would for any turn, and after `turn.completed` issues `turn_answer`:

```json
{"operation":"turn_answer","data":{"id":"styra-1"}}
```

```json
{"status":"ok","response":{"type":"answer","data":{
  "contract":"files",
  "value":{"contract":"files","value":[{"path":"src/auth.rs","line":12}]},
  "source":"…the agent message it was parsed from…"}}}
```

`turn_answer` parses the session's most recent agent message, so it answers a
live interaction and a stored session alike. Its optional `contract` field
overrides the recorded one, which is how an answer is re-read as another shape
and how a turn sent untyped is typed after the fact. Without it, a session that
has never had a typed turn is an error rather than a guess.

The value is tagged by the contract that produced it, so a client dispatches on
the answer alone. `line` and `column` in a `files` answer are 1-based and absent
when the agent named none — the difference between naming a file and naming a
position in it.

A reply that did not satisfy its contract is an answer too, not an error in
place of one: `value` is absent, `error` says what was wrong, and `source`
still carries what the agent said.

```json
{"status":"ok","response":{"type":"answer","data":{
  "contract":"json",
  "error":"the answer block is not valid JSON",
  "source":"I had trouble with that one."}}}
```

Only the absence of any reply to read — a session that has not answered yet, or
was never sent a typed turn — is a protocol error, since there is nothing to
return. An agent that answered well but framed it badly has produced something
worth showing, and a client handed only the complaint could not show it.

A contract also survives the durable input queue: `queue_message` takes the same
`SendMessage` as `send_message`, and `queued_messages` and `take_queued_message`
return `{"text":…,"contract":…}` objects rather than bare strings. A queue file
written before contracts existed is an array of strings and still loads, as
untyped messages.

## Compatibility

The Serde definitions in this directory are the structural wire contract.
Operation names, response type names, field names, enum spellings, and framing
must not be changed accidentally. Serialization-shape tests live beside the
definitions.

The protocol currently has no explicit version field. Incompatible changes
therefore require coordinated client and server updates.

`Request` uses `deny_unknown_fields`, so a new client's request against an older
server is refused rather than silently misread. New fields are therefore added
as optional, with an absent value meaning what the protocol meant before it
existed: no `contract` is an ordinary untyped turn, exactly as every turn was.
