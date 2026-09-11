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
styra --standalone            # run the server in this process, with no daemon
styra shell                   # choose and attach to a live sandbox shell
styra shell --session ID      # attach to that live session's shell
```

The client starts the daemon automatically if needed. `--socket PATH` selects
another Unix socket. `styra-server [--store DIR] [--socket PATH]` runs the
server directly. State defaults to `$XDG_STATE_HOME/styra` (or
`~/.local/state/styra`); the socket defaults to
`$XDG_RUNTIME_DIR/styra/styra.sock`.

`--standalone` skips the socket entirely and runs the server in the client's own
process. Its state lives separately at `$XDG_STATE_HOME/styra-standalone` (or
`~/.local/state/styra-standalone`), so it never shares the daemon's default
store. No daemon is started and no other client can attach; interactions end
when the interface exits, leaving their Sessions for later standalone runs.
Only one standalone process may own that store at a time.

`--workspace` is writable in the sandbox at its canonical host path.
`--template NAME` is repeatable and ordered; later templates override conflicts.
`--network` grants networking for that launch layer. These can be refined before
the first message in the details view.

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
| `B` | branch from history through, or only, the selected entry; opens the branch and leaves the source running |
| `b` | follow the selected `branch` marker to the Session it names |
| `!` | open this live session's sandbox shell in the configured terminal |
| `a` / `A` / `V` | live interactions / sessions in this Workspace / Workspaces |
| `Ctrl+A` | go to the next interaction that went idle unseen |
| `W` | enable or disable linked-worktree creation for future launches |
| `L` | choose provider, model, and effort |

Sending a message to a stopped or viewed Session automatically attempts native
provider resume. `a` opens the live-interaction list; there, `w` switches
current/all-Workspace scope, `j`/`k` selects, and `D` deletes a stopped
interaction (the durable Session remains). The interaction under the cursor is
loaded once the cursor rests on it, so the list can be crossed without waiting
for every row on the way; the row being loaded says so, and any other key acts
on it as soon as it arrives. An interaction that went idle away from every
client's screen, and has not been focused since, is marked `NEWLY IDLE`; the
footer counts those rows until they are focused. Finishing a turn while a client
is showing it is not one of them — you watched it happen.

`Ctrl+A` goes straight to the next such interaction, wherever it is: with the
list open it moves the cursor there (revealing all Workspaces if it lives in
another one), and with the list closed it makes that interaction current and
opens the list around it. Pressing it repeatedly walks every waiting
interaction and wraps back to the first.

## Read the session

| Key | View or action |
| --- | --- |
| `r`, `l`, `t`, `d` | raw wire records, client/server log, transcript, Workspace/interaction details; press again for events |
| `Q` | quota readings observed by the server (`R` there: keep at it after a rate limit) |
| `f` | files associated with the selected event (or the whole session) |
| `F` | open a file the selected event cites (`path:line` included) |
| `X` | typed answer from the last turn |
| `p` / `P` | toggle side preview / full-screen preview |
| `v` / `C` | pretty versus diff preview / preview newest command |
| `y` | copy the selected item to the clipboard |
| `c` | show conversation events only (events and transcript) |

In the event list: `j`/`k` moves by line, `J`/`K` (or arrows) moves by event,
`g`/`G` jumps first/last, `Space`/`Enter`/`o` folds the selected event, `O`
expands only it, `z R` expands all, `z M` collapses all, and `m` hides/shows
minor events. `PgUp`/`PgDn` scrolls a preview. Raw, log, quota, and transcript
use `j`/`k` plus `g`/`G` to navigate. In the raw view, `v` switches between
Styra's captured app-server wire traffic and the provider's native persisted
session JSONL (when the provider still has it).

`F` — in the event list, transcript, or full-screen preview — lists every file
the selected event names that exists on this host, including citations of the
form `monitor.c:484`. `j`/`k` and `g`/`G` move, `Enter` opens the highlighted
file in the configured opener (`nvim` by default), and `q` or `Esc` cancels.
The cited line is shown but not jumped to: only the opener knows how it is
asked to.

In Files: `e` opens the selected path in the configured opener, `a` switches
focused-event/all-session files, `p` previews, `y` copies its path, and `J`/`K`
changes the source event. In Typed answer: `T`, `L`, `F`, `J` re-read the last
answer as text, lines, files, JSON; `R` uses the turn's original requested
shape; `e` opens a selected file; `y` copies.

## Wait out a rate limit

When a plan window runs dry the provider refuses the turn and the agent process
ends, leaving the session stopped with your last message unanswered. Press `Q`
for the quota view and `R` there to have the session keep at it: once that
window turns over, the server resumes the session and asks it the same turn
again. The bottom border of that view says which way `R` is set, and the
readings above it say which window is full and when it resets.

The waiting is the server's, so it holds while Styra is closed and applies to
whichever client next opens the session. `R` is answered for one session, is
remembered with it, and stays on afterwards — a session that runs into the next
window is waited out again without being asked twice. Press `R` again to stop.

The turn goes out exactly as it did the first time, framing and all, so a turn
that asked its reply for a shape asks for it again, and the session comes back
in the same sandbox it was launched in. Three minutes are left after the
reported reset, since a request landing on the minute itself tends to be
refused for being early. A refusal that names no reset is left alone: there is
no minute to come back at, and Styra does not guess one. Whatever happens is
written to the session's log — the wait starting, the window coming back, and a
resume that failed — so a session you left stopped explains itself when you
return to it.

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
