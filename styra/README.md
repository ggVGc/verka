# Styra

Styra is a local server for interactive, isolated agent sessions plus a
terminal client. The server uses [Driva](../driva) to execute agents with
deny-by-default isolation, owns their machine-readable protocols and journals,
and exposes a versioned JSON API over a Unix domain socket. The `styra` TUI
uses only that API, so other local tools can create, steer, observe, stop, and
replay the same sessions without depending on the TUI.

Styra is the interactive counterpart to an [Orka](../orka) attempt: the same
isolation, the same raw-event-journal-as-truth stance, but steered turn by turn
by an operator rather than run to completion against a Linka node.

See [`DESIGN.md`](DESIGN.md) for the architecture and [`TASKS.md`](TASKS.md) for
the implementation plan.

## Usage

Start the server:

```sh
styra-server
```

Then start the TUI in another terminal:

```sh
styra [OPTIONS] [-- PROMPT]

  --socket <PATH>      Server socket (default: $XDG_CONFIG_HOME/styra/styra.sock)
  --profile <NAME>     Agent profile to launch (default: codex)
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (profiles may default this on)
  --view [<SESSION>]   Open a captured journal read-only instead of launching;
                       bare, browse sessions in the server's store and pick one
```

The server accepts `--store <DIR>` and `--socket <PATH>`. By default the store
is `$XDG_CONFIG_HOME/styra`, or `$HOME/.config/styra` when
`XDG_CONFIG_HOME` is unset. The socket defaults to `styra.sock` inside that
store and is created with mode `0600`.

## Socket API

Each connection carries one newline-terminated JSON request and one
newline-terminated JSON response. Requests carry `api_version` and an
`operation` tag. Successful responses use
`{"status":"ok","response":...}`; failures use
`{"status":"error","error":"..."}`.

Operations:

| Operation | Data | Result type |
| --- | --- | --- |
| `health` | none | `health` |
| `create_session` | profile, workspace, network, optional message | `session_created` |
| `send_message` | session id and message | `accepted` |
| `updates` | session id and `after` cursor | `updates` |
| `stop_session` | session id | `accepted` |
| `list_stored_sessions` | none | `stored_sessions` |
| `stored_session` | session id | `stored_session` |
| `transcript` | session id | `transcript` |

The update stream is cursor-based. Clients pass the last observed sequence as
`after`; the response supplies `next`. Repeating a request with the same cursor
is safe, and different clients can observe a session independently.

For example, a shell tool can check the server with `socat`:

```sh
printf '%s\n' '{"api_version":"v1","operation":"health"}' \
  | socat - UNIX-CONNECT:"${XDG_CONFIG_HOME:-$HOME/.config}/styra/styra.sock"
```

The Rust wire types are in `styra::api`, and the blocking client used by the
TUI is `styra::client::Client`.

Built-in profiles:

- `codex` — multi-turn session over the codex `app-server` JSON-RPC protocol;
  each submitted message starts a new turn in the same thread.
- `codex-exec` — one-shot `codex exec --json`; the first message is the prompt
  and the session ends when the turn completes.
- `claude` — multi-turn session over Claude Code's bidirectional `stream-json`
  mode; each submitted message starts a new turn in the same session.
  `claude:<model>` (e.g. `claude:opus`) pins a model.

Two focuses, like vim modes: list focus navigates and folds the event list,
input focus types into the message box. `i` or `Tab` enters input focus; `Esc`
or `Tab` returns to list focus.
