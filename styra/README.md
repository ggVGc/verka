# Styra

Styra is a local server for Workspaces containing interactive, isolated agent
Sessions, plus a terminal client. A Workspace groups related work against one
host directory. A Session is one durable provider conversation and journal. An
Interaction is the live process/protocol wrapper serving a Session while its
agent is running.

The server uses [Driva](../driva) for deny-by-default isolation and exposes a
JSON API over a Unix socket. The `styra` TUI uses only that API, so
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
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (providers may default this on)
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
newline-terminated JSON response. Requests carry an `operation` tag. Successful responses use
`{"status":"ok","response":...}`; failures use
`{"status":"error","error":"..."}`.

Operations:

| Operation | Data | Result type |
| --- | --- | --- |
| `health` | none | `health` |
| `create_workspace` | host path and optional name | `workspace_created` |
| `list_workspaces` | none | `workspaces` |
| `workspace` | Workspace id | `workspace` |
| `create_session` | Workspace id, provider/model/effort selection, network, optional message | `session_created` |
| `resume_session` | Session id, network, templates | `session_resumed` |
| `list_sessions` | Workspace id | `stored_sessions` |
| `send_message` | session id and message | `accepted` |
| `updates` | session id and `after` cursor | `updates` |
| `stop_interaction` | session id | `accepted` |
| `stored_session` | session id | `stored_session` |
| `shell` | live session id | `shell` (tmux executable and socket) |
| `list_interactions` | none | `interactions` |

The update stream is cursor-based. Clients pass the last observed sequence as
`after`; the response supplies `next`. Repeating a request with the same cursor
is safe, and different clients can observe a session independently. A resumed
Interaction seeds this stream with the Session's stored events and raw records.
The resume response supplies the boundary cursor for the initiating client,
which already rendered that journal; later clients attach from zero and receive
the complete history followed by new provider output.

For example, a shell tool can check the server with `socat`:

```sh
printf '%s\n' '{"operation":"health"}' \
  | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/styra/styra.sock"
```

The server and its client interface are the `styra-server` crate (library
`styra_server`); the `styra` TUI is a separate crate depending on it. The Rust
wire types are in `styra_server::api`, and the blocking client used by the TUI
is `styra_server::Client`.

The launch picker selects a provider, model, and effort. Providers:

- `codex` — multi-turn session over the codex `app-server` JSON-RPC protocol;
  each submitted message starts a new turn in the same thread.
- `claude` — multi-turn session over Claude Code's bidirectional `stream-json`
  mode; each submitted message starts a new turn in the same session.

Every selection pins a model and an effort. Codex accepts `minimal` through
`xhigh` as its `model_reasoning_effort`; Claude Code accepts `low` through `max`
as `--effort`. Until an operator saves another choice, new sessions begin on
Codex's declared defaults. Switching providers in the picker initially selects
that provider's defaults. The selection is recorded with the session, so stored
state always says which provider, model, and effort ran.

The TUI's start screen — no session launched yet, whether at startup or after a
reset with `S` — picks all three interactively: `L` (or `Ctrl+L` from the message
box) opens a picker with an agent, model, and effort column, each listing that
agent's own catalog — for Claude Code the full ids Anthropic lists as active, so a
session records the exact model it ran on. Every row is a concrete choice, and
switching agents lands on that agent's declared default. Applying the choice
saves it in `$XDG_CONFIG_HOME/styra/defaults.json` (or
`$HOME/.config/styra/defaults.json`) and uses it for this and future Styra
starts, until another choice is saved. The first message still starts the
agent. A model outside the current catalog can be retained from stored state,
and the picker carries it as a final row rather than dropping it.

Once a session runs, its status line (every view's top border) names the model
and effort in use: each agent reports what it resolved as it starts a session, so
that report is what is shown. A value that is still only what the selection
asked for (before the agent's report, or Claude Code's effort, which it never
reports) is dimmed.

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
Interaction while retaining its Session. `F` resumes that stopped Session with
the provider's native conversation id: Codex uses `thread/resume`, while Claude
Code starts with `--resume`. Styra preserves the provider's own state directory
for this purpose. If the provider has removed its session—or an older Styra
journal predates native-id capture—the raw journal remains viewable, but resume
returns an error.
