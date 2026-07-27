# Styra

Styra is a local server for Workspaces containing interactive, isolated agent
Sessions, plus a terminal client. A Workspace groups related work against one
host directory. A Session is one durable provider conversation and journal. An
Interaction is the live process/protocol wrapper serving a Session while its
agent is running.

The server uses [Driva](../driva) for deny-by-default isolation and exposes a
versioned JSON API over a Unix socket. The `styra` TUI uses only that API, so
other local tools can create, steer, observe, stop, and replay the same Sessions.

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
styra shell --session <ID>

  --socket <PATH>      Server socket (default: $XDG_RUNTIME_DIR/styra/styra.sock)
  --profile <NAME>     Agent to launch, as provider:model/effort; a shorter
                       provider[:model][/effort] takes that agent's declared
                       defaults (default: codex); the start screen can change it
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (profiles may default this on)
  --view [<SESSION>]   Open a captured journal read-only instead of launching;
                       bare, browse sessions in the server's store and pick one
```

Every live Interaction also owns a detached `/bin/sh` in tmux inside the same
Bubblewrap sandbox as its agent. Attach with `styra shell --session <ID>`.
Stopping the Interaction ends the agent and tmux but preserves its Session,
journal, and Workspace. The agent remains on its original piped machine
protocol, so shell traffic never enters the raw event journal.

The server accepts `--store <DIR>` and `--socket <PATH>`. By default, durable
Workspaces and Sessions live under `$XDG_STATE_HOME/styra`, or
`$HOME/.local/state/styra`
when `XDG_STATE_HOME` is unset. The socket lives independently at
`$XDG_RUNTIME_DIR/styra/styra.sock`. Default Styra directories use mode `0700`
and the socket uses mode `0600`.

The durable layout is:

```text
workspaces/<WORKSPACE-ID>/
  workspace.json
  sessions/<SESSION-ID>/
    session.json
    journal.jsonl
    diagnostics.log
```

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
| `create_workspace` | host path and optional name | `workspace_created` |
| `list_workspaces` | none | `workspaces` |
| `workspace` | Workspace id | `workspace` |
| `create_session` | Workspace id, profile, network, optional message | `session_created` |
| `list_sessions` | Workspace id | `stored_sessions` |
| `send_message` | session id and message | `accepted` |
| `updates` | session id and `after` cursor | `updates` |
| `stop_interaction` | session id | `accepted` |
| `stored_session` | session id | `stored_session` |
| `transcript` | session id | `transcript` |
| `shell` | live session id | `shell` (tmux executable and socket) |
| `list_interactions` | none | `interactions` |

`list_stored_sessions` remains as the compatibility operation for Sessions
created before Workspaces existed. They appear in the TUI under the synthetic,
read-only `Legacy sessions` Workspace.

The update stream is cursor-based. Clients pass the last observed sequence as
`after`; the response supplies `next`. Repeating a request with the same cursor
is safe, and different clients can observe a session independently.

For example, a shell tool can check the server with `socat`:

```sh
printf '%s\n' '{"api_version":"v2","operation":"health"}' \
  | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/styra/styra.sock"
```

The server and its client interface are the `styra-server` crate (library
`styra_server`); the `styra` TUI is a separate crate depending on it. The Rust
wire types are in `styra_server::api`, and the blocking client used by the TUI
is `styra_server::Client`.

A profile name is a launch selection, `provider:model/effort`. Providers:

- `codex` — multi-turn session over the codex `app-server` JSON-RPC protocol;
  each submitted message starts a new turn in the same thread.
- `codex-exec` — one-shot `codex exec --json`; the first message is the prompt
  and the session ends when the turn completes.
- `claude` — multi-turn session over Claude Code's bidirectional `stream-json`
  mode; each submitted message starts a new turn in the same session.

Every profile pins a model and an effort. `:<model>` names the model
(`claude:claude-opus-5`, `codex:gpt-5.6-sol`, or any other id the agent knows) and
`/<effort>` the reasoning effort — codex accepts `minimal` through `xhigh` as its
`model_reasoning_effort`, Claude Code `low` through `max` as `--effort`. Omitting
either takes that agent's declared default (`codex` → `codex:gpt-5.6-sol/high`,
`claude` → `claude:claude-opus-5/high`) rather than leaving it to the agent's own
configuration, and the profile then names itself in full. The selection is what a
session's journal records, so a stored session always says which agent, model, and
effort actually ran.

The TUI's start screen — no session launched yet, whether at startup or after a
reset with `S` — picks all three interactively: `L` (or `Ctrl+L` from the message
box) opens a picker with an agent, model, and effort column, each listing that
agent's own catalog — for Claude Code the full ids Anthropic lists as active, so a
session records the exact model it ran on. Every row is a concrete choice, and
switching agents lands on that agent's declared default. Applying the choice only
records it; the first message still starts the agent. A model outside the catalog reaches a session through
`--profile`, and the picker carries it as a final row rather than dropping it.

Once a session runs, its status line (every view's top border) names the model
and effort in use: each agent reports what it resolved as it starts a session, so
that report is what is shown — even when nothing was pinned. A value that is
still only what the launch asked for (before the agent's report, or Claude Code's
effort, which it never reports) is dimmed; with nothing pinned and nothing
reported yet, the line reads `default model`.

Each profile's agent binary is located on the server's own `PATH` when the
session is created, and the session launches that resolved path: the sandbox
gets a fixed system `PATH`, so a bare name installed under your home (Claude
Code's `~/.local/bin`) would not resolve inside it. An agent that is not
installed fails the `create_session` request with a clear error instead of
dying inside the sandbox.

Two focuses, like vim modes: list focus navigates and folds the event list,
input focus types into the message box. `i` or `Tab` enters input focus; `Esc`
or `Tab` returns to list focus.

`V` first chooses a Workspace, then a Session within it. This only changes what
the client views; it never stops an outgoing Interaction. `A` lists all live
Interactions with their Workspace and Session identities. `s` stops the current
Interaction while retaining its Session. `F` renders the current Session into
the message box for an explicit new Session in the same Workspace; the new
Session is not created until the operator sends that opening message.
