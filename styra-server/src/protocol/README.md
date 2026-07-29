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

A resumed interaction seeds its update stream from stored history. The resume
response's `updates_after` cursor lets the resuming client skip history it has
already rendered; a newly attaching client starts from zero.

## Compatibility

The Serde definitions in this directory are the structural wire contract.
Operation names, response type names, field names, enum spellings, and framing
must not be changed accidentally. Serialization-shape tests live beside the
definitions.

The protocol currently has no explicit version field. Incompatible changes
therefore require coordinated client and server updates.
