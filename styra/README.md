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
styra shell [--session <ID>]

  --socket <PATH>      Server socket (default: $XDG_RUNTIME_DIR/styra/styra.sock)
  --workspace <DIR>    Host directory mounted writable as the agent workspace
  --network            Permit agent networking (providers may default this on)
  --template <NAME>    Layer a Driva execution template onto the sandbox;
                       repeatable, later names win on conflict
  --view [<SESSION>]   Open a captured journal read-only instead of launching;
                       bare, browse sessions in the server's store and pick one
```

Every live Interaction also owns a detached `/bin/sh` in tmux inside the same
Bubblewrap sandbox as its agent. Run `styra shell` to browse live sessions and
attach, or use `styra shell --session <ID>` to attach directly.
From the TUI, press `!` to open that shell in a new terminal window. Styra
prefers the emulator named by `$TERMINAL`, then uses `TERM_PROGRAM` and `$TERM`
as hints before trying common installed terminal emulators.
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

`workspace.json` also holds the Workspace's standing launch policy: the Driva
templates, extra mounts, and network permission every launch there starts from.
An individual launch adds its own on top — the driva view (`d`) edits that half
and shows which grants come from which layer — so `W` keeps a policy for the
Workspace and every client launching there picks it up, while `D` keeps one as
this client's own starting point. A launch that asks for nothing runs under
exactly the Workspace's policy; `I` makes one ignore it entirely, which is how a
single interaction drops a grant the Workspace makes.

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
| `create_session` | Workspace id, provider/model/effort selection, this launch's own policy, optional message | `session_created` |
| `plan_session` | Workspace id and the same launch inputs, creating nothing | `session_plan` |
| `list_templates` | Workspace id | `templates` |
| `resume_session` | Session id and this launch's own policy | `session_resumed` |
| `convert_session_provider` | Session id | `session_converted` (a new sibling Session, resumable under Styra's other interactive provider; sugar over `branch_session` with no cutoff and the other provider) |
| `branch_session` | Session id, optional history cutoff (`at_ms`), optional destination provider | `session_branched` (a new sibling Session seeded with the history up to the cutoff, or all of it) |
| `rename_session` | Session id and optional name | `session_renamed` |
| `update_session_notes` | Session id and plain-text notes | `session_notes_updated` |
| `update_workspace_notes` | Workspace id and plain-text notes | `workspace_notes_updated` |
| `set_workspace_launch` | Workspace id and the standing launch policy every launch there starts from | `workspace_launch_updated` |
| `list_sessions` | Workspace id | `stored_sessions` |
| `send_message` | session id and message | `accepted` |
| `updates` | session id and `after` cursor | `updates` |
| `interrupt_interaction` | session id | `accepted` |
| `stop_interaction` | session id | `accepted` |
| `close_interaction` | session id | `accepted` |
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

While an idle Codex interaction is live, submit `/cd <directory>` in the
message box to change its directory for subsequent turns. Relative paths are
resolved from the Workspace root; absolute paths are accepted only when they
remain inside that Workspace. The Workspace's mount stays unchanged, and
Claude Code interactions currently require a new/resumed interaction instead.

Each profile's agent binary is located on the server's own `PATH` when the
session is created, and the session launches that resolved path: the sandbox
gets a fixed system `PATH`, so a bare name installed under your home (Claude
Code's `~/.local/bin`) would not resolve inside it. An agent that is not
installed fails the `create_session` request with a clear error instead of
dying inside the sandbox.

Two focuses, like vim modes: list focus navigates and folds the event list,
input focus types into the message box. `i` or `Tab` enters input focus; `Esc`
or `Tab` returns to list focus.

`V` first chooses a Workspace, then a Session within it. `a` goes directly to
the Session picker for the current Workspace. These only change what the client
views; they never stop an outgoing Interaction. `A` lists all live
Interactions with their Workspace and Session identities; in that list `d`
closes the selected Interaction — the server stops it and forgets it, leaving
the Session as stored history like any other one on disk (navigate with `j`/`k`
or the arrow keys there). `s` stops the current
Interaction while retaining its Session. Sending a new message to a stopped,
ended, or merely-viewed Session resumes it automatically with the provider's
native conversation id: Codex uses `thread/resume`, while Claude Code starts
with `--resume`. Styra preserves the provider's own state directory for this
purpose. If the provider has removed its session—or an older Styra journal
predates native-id capture—the raw journal remains viewable, but resume
returns an error and the message is not lost, ready to retry.

Sessions receive a display name from their first prompt (normalized and
truncated locally; no extra model call is made). In a Session picker, press
`r` to edit that name; saving an empty value clears it. Names need not be
unique and never replace the immutable Session id used by the API and store.

Workspaces and Sessions can each carry durable, multiline notes. Press `E` in
the main view to read and write them without leaving the session: the editor
floats over the current view on this Session's notes, `Tab` moves between
Session and Workspace notes, `Ctrl+S` saves both, and `Esc` closes without
saving. While either set of notes is non-empty the view's bottom border carries
a `✎ notes` marker, so notes written to be found later are visible without
opening anything. Before the first message there is no Session yet, so the
editor opens on the Workspace notes alone.

The Workspace and Session pickers show the same notes in a yellow pane labelled
with its scope, where `e` edits the notes of the highlighted row on the same
`Ctrl+S`/`Esc` terms. Empty text clears the notes. Workspace notes live in
`workspace.json`, while Session notes live in that Session's `session.json`.
