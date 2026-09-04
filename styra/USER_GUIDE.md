# Styra user guide

Styra is a local terminal UI for durable, isolated coding-agent conversations.
A **Workspace** is a host directory and its saved launch policy; a **Session**
is one durable provider conversation; a live **Interaction** runs that Session
inside Driva's sandbox. Sessions and journals survive stopping, quitting, and
daemon restarts.

## Start, stop, and attach

```sh
styra                         # open/select the workspace for the current directory
styra --workspace /path/to/project -- "implement the parser"
styra --network --template rust -- "run the tests"
styra --view                  # browse saved sessions read-only
styra --view SESSION_ID       # view one saved session read-only
styra -d                      # start the local daemon
styra --stop                  # stop the daemon and its live interactions
styra shell                   # choose and attach to a live sandbox shell
styra shell --session ID      # attach to that live session's shell
```

The client starts the daemon automatically if needed. `--socket PATH` selects
another Unix socket. `styra-server [--store DIR] [--socket PATH]` runs the
server directly. State defaults to `$XDG_STATE_HOME/styra` (or
`~/.local/state/styra`); the socket defaults to
`$XDG_RUNTIME_DIR/styra/styra.sock`.

`--workspace` is writable in the sandbox at its canonical host path.
`--template NAME` is repeatable and ordered; later templates override conflicts.
`--network` grants networking for that launch layer. These can be refined before
the first message in the Driva view.

## First turn and model choice

At the blank start screen, choose a provider, model, and effort with `L` (or
`Ctrl+L` in the editor), then type a message and press `Enter`. The first
message launches the session. `D` in the picker also saves that selection as
the default; `Enter` only uses it now. Codex and Claude Code are available when
their executables are on the server's `PATH`.

During an idle live session, `L` changes the model selection for the next turn.
`Ctrl+T` asks the message's reply to have a shape: text, lines, files, or JSON.
`/cd DIR` changes the working directory of an idle Codex interaction; relative
paths are from the Workspace root and absolute paths must remain inside it.

## Everyday controls

`?` opens the in-app key reference. `i` or `Tab` enters the message editor;
`Esc` or `Tab` returns to the event list. `q` quits the client without stopping
live interactions.

| Key | Use |
| --- | --- |
| `Enter` / `Alt+Enter` | send message / insert editor newline |
| `Up`/`Down`, `Ctrl+W` | message history; delete previous word |
| `s` / `S` | interrupt the active turn / stop its interaction |
| `n` / `N` | new session / stop then start a new session |
| `b` | branch a session from the selected event/history point |
| `!` | open this live session's sandbox shell in a new terminal |
| `a` / `A` / `V` | live interactions / sessions in this Workspace / Workspaces |
| `W` | enable or disable linked-worktree creation for future launches |
| `L` | choose provider, model, and effort |

Sending a message to a stopped or viewed Session automatically attempts native
provider resume. `a` opens the live-interaction list; there, `w` switches
current/all-Workspace scope, `j`/`k` selects, and `D` deletes a stopped
interaction (the durable Session remains).

## Read the session

| Key | View or action |
| --- | --- |
| `r`, `l`, `t`, `d` | raw provider records, client/server log, transcript, Driva policy; press again for events |
| `Q` | quota readings observed by the server |
| `f` | files associated with the selected event (or the whole session) |
| `X` | typed answer from the last turn |
| `p` / `P` | toggle side preview / full-screen preview |
| `v` / `C` | pretty versus diff preview / preview newest command |
| `y` | copy the selected item to the clipboard |
| `c` | show conversation events only (events and transcript) |

In the event list: `j`/`k` moves by line, `J`/`K` (or arrows) moves by event,
`g`/`G` jumps first/last, `Space`/`Enter`/`o` folds the selected event, `O`
expands only it, `z R` expands all, `z M` collapses all, and `m` hides/shows
minor events. `PgUp`/`PgDn` scrolls a preview. Raw, log, quota, and transcript
use `j`/`k` plus `g`/`G` to navigate.

In Files: `e` opens the selected path in the configured editor, `a` switches
focused-event/all-session files, `p` previews, `y` copies its path, and `J`/`K`
changes the source event. In Typed answer: `T`, `L`, `F`, `J` re-read the last
answer as text, lines, files, JSON; `R` uses the turn's original requested
shape; `e` opens a selected file; `y` copies.

## Give the agent a file

In the editor, `Ctrl+F` opens a path prompt. Type a host path, use `Tab` to
complete, and press `Enter` to insert the sandbox-visible path into the
message. If no existing mount carries that path, choose `r` to add a read-only
mount, `w` for read/write, or `n` to insert without a mount. The path must
exist. New grants affect the next launch/resume; a running sandbox cannot gain
mounts, so Styra tells you when the inserted path is unreachable now.

## Control isolation and reusable launch policy

Press `d` before starting an interaction. The view has two layers: the
Workspace policy (shared and durable) and this interaction's additions. `Tab`
switches focused layer; `j`/`k` selects a mount.

| Key | Focused-layer change |
| --- | --- |
| `w` | cycle network permission |
| `T` | select Driva templates |
| `m` / `x` | add a mount / remove selected mount |
| `I` | make this interaction add to, or ignore, Workspace policy |
| `U` | promote this interaction's additions into Workspace policy |
| `D` | save this interaction's additions as new-client defaults |

Workspace edits take effect for future launches everywhere in that Workspace.
Interaction edits apply only to its next launch/resume unless promoted. Existing
live sandboxes are immutable.

## Git checkout association and linked worktrees

These are related but independent Workspace features:

| Feature | How it is enabled | What a future interaction receives |
| --- | --- | --- |
| **Git checkout association** | When the TUI creates a Workspace, it finds the nearest enclosing checkout and records its canonical root automatically. | The checkout is mounted read-only at its host path; its Git metadata/common directory is writable, so Git can operate on that checkout. |
| **Linked worktrees** | Press `W` to opt the current Workspace in. | A writable worktree parent at `/tmp/styra/worktrees`, writable shared Git metadata, and (for Codex) `create_worktree`. |

The automatic association is visible in Workspace metadata but has no TUI edit
screen yet. A non-TUI client can explicitly replace or clear it through the
local socket API; `git_repository` may name the checkout or any directory in
it, and Styra stores its root:

```sh
printf '%s\n' \
  '{"operation":"set_workspace_git_repository","data":{"workspace_id":"WORKSPACE_ID","git_repository":"/path/in/checkout"}}' \
  | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/styra/styra.sock"
```

Use `"git_repository":null` to clear the association. The checkout must exist
and be inside a Git repository.

Worktree creation is discovered from the **Workspace host directory**, not from
the optional checkout association. It works only when that directory is inside
a Git working tree. `W` affects future launches only and does not delete
existing worktrees. In an enabled Codex session, ask the agent to call
`create_worktree` with a valid new branch name (for example
`feature/search-index`); Styra creates the branch and checkout below the
Workspace store and returns its sandbox path under `/tmp/styra/worktrees`.

## What Styra records and does not do

Each Session stores metadata, a raw JSONL journal, and diagnostics under its
Workspace. The UI can be reopened or multiple local clients can observe the
same Session. Styra is local-only (Unix socket, no TCP listener), does not
review or submit work, and does not recreate lost provider context; it relies
on the provider's native resume support.
