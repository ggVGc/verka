# Styra user guide

Styra is a local terminal UI for durable, isolated coding-agent conversations.
A **Workspace** is a host directory and its saved launch policy; a **Session**
is one durable provider conversation; a live **Interaction** runs that Session
inside Driva's sandbox. Sessions and journals survive stopping, quitting, and
daemon restarts.

## Start, stop, and attach

```sh
styra                         # open/select the workspace for the current directory
styra --skip-workspace-list   # go straight to the interaction view, live list open
styra --workspace /path/to/project -- "implement the parser"
styra --network --template rust -- "run the tests"
styra --view                  # browse saved sessions read-only
styra --view SESSION_ID       # view one saved session read-only
styra -d                      # start the local daemon
styra --stop                  # stop the daemon and its live interactions
styra --standalone            # run the server in this process, with no daemon
styra shell                   # choose and attach to a live sandbox shell
styra shell --session ID      # attach to that live session's shell
styra worktrees               # list worktrees and the sessions working in each
styra clean-worktrees         # delete the worktrees of finished, committed sessions
styra clean-worktrees --all   # the same, across every Workspace
```

The client starts the daemon automatically if needed. `--socket PATH` selects
another Unix socket. `styra-server [--store DIR] [--socket PATH]` runs the
server directly. State defaults to `$XDG_STATE_HOME/styra` (or
`~/.local/state/styra`); the socket defaults to
`$XDG_RUNTIME_DIR/styra/styra.sock`.

A plain `styra` in a directory that already has a Workspace opens that
Workspace. In a directory without one, it opens the Workspace list with a
popup asking whether to create a Workspace for the current directory. `Enter`
or `y` creates it and starts in it; `Esc` or `n` closes the popup, leaving the
list to open an existing Workspace from.

`--skip-workspace-list` never stops at that list. In a directory without a
Workspace it goes straight to the interaction view: on the live interaction a
Workspace would land on if any is running, in whichever Workspace that is,
and otherwise on a blank screen in the Workspace accessed most recently (a
Workspace for the current directory is created only when there are none at
all). Whenever any interaction is live, the live-interaction list (`a`) opens
with it.

`--standalone` skips the socket entirely and runs the server in the client's own
process. Its state lives separately at `$XDG_STATE_HOME/styra-standalone` (or
`~/.local/state/styra-standalone`), so it never shares the daemon's default
store. No daemon is started and no other client can attach; interactions end
when the interface exits, leaving their Sessions for later standalone runs.
Only one standalone process may own that store at a time.

`--workspace` is writable in the sandbox at its canonical host path — unless the
Workspace makes linked worktrees, in which case the interaction gets one of
those instead (see "Git checkout association and linked worktrees").
`--template NAME` is repeatable and ordered; later templates override conflicts.
`--network` grants networking for that launch layer. These can be refined before
the first message in the details view.

## First turn and model choice

At the blank start screen, choose a provider, model, and effort with `Ctrl+L`,
then type a message and press `Enter`. The first
message launches the session. The picker is one list of whole
`agent:model/effort` triples and is typed at rather than stepped through: the
letters of a triple's name in order — `chk45` for
`claude:claude-haiku-4-5-20251001`, `opus5/max` for `claude:claude-opus-5/max` —
narrow the list to it. `↑`/`↓` (or `Ctrl+P`/`Ctrl+N`) move through what is left
and `PgUp`/`PgDn` cross it a page at a time,
`Enter` launches on the selected row, `Ctrl+W` takes back one part of what was
typed (`claude:claude-opus-5/high` → `claude:claude-opus-5/`), and `Esc` clears
the query before it closes the picker. `Ctrl+D` in the picker also saves that selection as the
default; `Enter` only uses it now. Codex and Claude Code are available when
their executables are on the server's `PATH`.

During an idle live session, `Ctrl+L` changes the model selection for the next turn.
The agent itself is fixed while a process is up — the picker then lists only
that agent's triples and its title reads `agent fixed`. Once the interaction is stopped or has ended, the whole picker reopens:
the next message resumes the Session and the resume launches under whatever is
chosen, the agent included. Choosing a different agent for a stopped Session
converts its history into that agent's format as a sibling Session, and the
view follows the sibling — the conversation comes along, and both Sessions keep
a marker naming the other.
`Ctrl+T` asks the message's reply to have a shape: text, lines, files, or JSON.
`Ctrl+R` in the message editor opens the system's default audio input and starts
recording; press `Ctrl+R` again to stop. `styra-server` transcribes
the temporary recording with a local Whisper model and the text is inserted
into the draft —
no agent provider is involved and no quota is spent. Recording needs no
external tool — `styra` opens the device itself — but the model has to be
fetched once with `styra-transcribe --download-model` — recording before that reports the model
as missing rather than starting a download nobody asked for. The default model
is Whisper `large-v3-turbo-q5_0`: large-v3-turbo accuracy with a roughly 548 MB
quantized model suitable for an integrated GPU. `STYRA_WHISPER_MODEL` selects
another model such as `base`, `tiny-q5_1`, or `small`, and applies to both the
download and the transcription, so set it for both. A normal build uses the
CPU. Build the workspace with `--features transcription/vulkan` to use a Vulkan
GPU (including AMD GPUs), or with `--features transcription/rocm` to use an
installed ROCm toolchain. An accelerated build selects the GPU automatically;
`STYRA_WHISPER_DEVICE=cpu` forces its CPU fallback and `gpu` requires an
accelerated build.
`styra-transcribe FILE` prints the transcript of a file on its own, without a
server or a session.
`/cd DIR` changes the working directory of an idle Codex interaction; relative
paths are from the Workspace root and absolute paths must remain inside it.

## Everyday controls

`?` opens the in-app key reference for whichever window is showing — the event
list, the session or Workspace picker, the launcher, the interaction list, or
any of the modal choosers — so the screens themselves carry no strip of
shortcuts along their top. `i` or `Tab` enters the message editor;
`Esc` or `Tab` returns to the event list. The other views (raw, log, quota,
transcript, details, files, answer, preview) stack over the event list, and
`Esc` in any of them, with no prompt or chooser open, goes straight back to it.
`q` quits the client without stopping live interactions.

| Key | Use |
| --- | --- |
| `Enter` / `Ctrl+Enter` / `Alt+Enter` | send message / send it in a new Git workspace and branch (later in a Session, as `W` does, then sends; refused while the agent is mid-turn) / insert editor newline |
| `Ctrl+R` (message editor) | start/stop recording and insert its transcript |
| `W` (existing session) | create and associate a linked workspace and branch, then restart the interaction in it; reports when one already exists |
| `Up`/`Down`, `Ctrl+W` | message history; delete previous word |
| `Ctrl+N` (message editor) | add another box to the message after the current one and type in it; the boxes are sent together, each as its own paragraph |
| `Esc` (several boxes) | choose between the boxes: `j`/`k` move, `Enter`/`i` type in the chosen box, `d` deletes it, `Ctrl+N` adds one after it, `Esc` again leaves the editor with the boxes kept |
| `s` / `S` | interrupt the active turn / stop its interaction |
| `n` / `N` / `Ctrl+N` | go to a newly idle interaction, or the next running one / cycle actively running interactions / start a new session in the current Session's checkout when it has one; pressed again on that still-blank screen, it starts at the Workspace root instead |
| `B` | branch from history through, or only, the selected entry; opens the branch and leaves the source running |
| `Enter` / `b` | follow the selected `branch` marker to the Session it names |
| `!` | open this live session's sandbox shell in the configured terminal |
| `~` | open a host shell in the interaction's working directory, in the configured terminal |
| `a` / `A` / `V` | live interactions / sessions in this Workspace / Workspaces |
| `v` | overview: every running and idle interaction as a grid of tiles |
| `w` (not in details) | worktrees and the sessions in each; `Enter` opens the selected session |
| `Ctrl+L` | choose provider, model, and effort (one list, typed at) |
| `Ctrl+G` | turn auto-commit on/off for this interaction (also in the message editor) |

The message box — its boxes, which one you were typing in, and the messages
sent from it for `Up`/`Down` — is stored by the server with each Session, so
switching interactions shows each one's own draft, and it is still there after
closing the client or from another client. The blank start screen has no
Session to store a draft with, so one left there is not kept; the message sent
from it becomes the first history entry of the Session it starts.

Sending a message to a stopped or viewed Session automatically attempts native
provider resume. In the main interaction view, `T` edits the current
interaction's tags. `a` opens the live-interaction list; there, `w` switches
current/all-Workspace scope, `j`/`k` selects, `T` edits the selected
interaction's tags, `C` / `Z` marks it completed / abandoned and stops it (also
available directly in the event view), `c` shows or hides those rows again, and
`D` deletes a stopped
interaction (the durable Session remains). `/` filters the list as you type —
case-insensitively, by name, tag, branch, provider, or Workspace name. The
arrows still move the cursor while you type. `Enter` opens the interaction under
the cursor, and `Esc` clears the filter (a second `Esc` closes the list).
`Ctrl+T` opens the tag list used by `T` to choose a tag filter instead: tick
tags with `Space` and `Enter` lists only interactions carrying every ticked
tag (no new tags can be added there; ticking none lists everything again). The
tag filter is named in the list's title and stays until changed.
The interaction under the cursor is
loaded once the cursor rests on it, so the list can be crossed without waiting
for every row on the way; the row being loaded says so, and any other key acts
on it as soon as it arrives. When an interaction is associated with a Git
checkout, its row also names the checked-out branch (or says `detached head`).
A branched interaction is listed beneath the one it was branched from, as in
the session picker. Branching (`B`) lists the new interaction straight away,
stopped as `branched, not started`, until a message resumes it.
An interaction that went idle away from every
client's screen, and has not been focused since, is marked `NEWLY IDLE`; the
event list's bottom border counts those rows, beside the `running/idle/stopped`
tally, until they are focused. Finishing a turn while a client
is showing it is not one of them — you watched it happen.

`v` opens the overview, which lays out every interaction that is running or
idle — in every Workspace, whatever the `a` list's scope or filter — as a grid
of tiles, so the whole fleet can be watched at once. Each tile names the
interaction, its Workspace and branch, its agent and model, and what it is
doing (with how long a turn has been running), carries the same `NEWLY IDLE`,
`RATE LIMITED` and uncommitted marks as the list, and ends with the last thing
the agent said. The grid takes as many columns as the terminal's width allows
and scrolls by rows when there are more tiles than fit. `h`/`j`/`k`/`l` or the
arrows move between tiles (in the overview `l` moves rather than opening the
launcher), `Tab`/`Shift-Tab` step to the next and previous tile, wrapping at
the ends, `g`/`G` jump to the first and last, and `Enter` opens the selected
interaction in the event list. `i` opens the message box for the selected
interaction, as it does on the event list, while the grid stays up behind it;
after sending or `Esc` you are back on the grid. `v` or `Esc` goes back without switching.
Stopped and completed interactions are left out; `a` still lists them.

An interaction that stops with uncommitted changes in its Git checkout — edits
or new files the agent left behind — is marked `UNCOMMITTED` in the list, and
`uncommitted changes` sits on the bottom border of the pane — beside
`all events` when every event is shown — for the interaction you are
attached to.
The checkout is read at the moment the agent stops working, so the mark
describes what that turn left; it clears the next time the interaction goes
idle. A workspace that is not in a repository says nothing either way.

Auto-commit is on by default for an interaction working in a linked worktree
of its own (see [Git checkout association and linked
worktrees](#git-checkout-association-and-linked-worktrees)), and off for one
working in the Workspace directory. While it is on, each time the interaction
goes idle the server stages everything `git status` reports in its checkout
and commits it.
The subject is the start of the agent's last message in the turn; the body
holds the message that started the turn, then every agent message in it. The
interaction's stream logs the commit's id and subject, or why it could not be
made — a missing Git identity or a refusing hook, say. A clean checkout, or a
workspace that is not in a repository, commits nothing. `Ctrl+G` turns it on
or off for the interaction you are attached to. The Session remembers your
choice, which overrides the default from then on — including after `W` moves
the Session into a worktree. The footer shows `^G auto-commit` while it is on.
Whatever `git status` sees is committed, so if you turn it on in a checkout you
edit yourself, your changes are committed alongside the agent's.

`A` opens the full Session list. In that list, `/` starts a case-insensitive
filter over the Session name and first prompt; `Esc` abandons the filter. `c`
shows or hides the Sessions marked completed or abandoned. `C` marks the
selected Session completed and `Z` marks it abandoned — given up on rather than
finished; each key, pressed on a Session already marked that way, unmarks it.
Neither touches a sealed Session. `n` leaves the list
without resuming anything and starts a new Session in the Workspace whose list
you were reading. `V` opens the Workspace list, which is typed at: every
printable key filters it over the Workspace name and host path, `Ctrl+W` drops the last
word, and `Esc` clears the filter before a second `Esc` backs out. The arrows
(or `Ctrl+J`/`Ctrl+K`) move and `Enter` opens. `Ctrl+N` enters the selected
Workspace on a new interaction, skipping its live work and Session list;
`Ctrl+C` creates a Workspace for the current directory; `Ctrl+R` changes the
selected Workspace's display name, and submitting a blank name restores its
directory-name fallback. `?` still opens the key reference.

`w` opens the worktree list: one row per session working in a linked
checkout, named by the checkout, with the path, branch and Workspace of the
selected row spelled out beneath. It is typed at like the Workspace list —
every printable key filters it over checkout, branch and session name — and
`Enter` opens the selected session, entering its Workspace if it is another
one. `Tab` switches between this Workspace and every Workspace, and the list
opens on the session you are viewing.

A Session you did not name yourself is named after what you asked for — a
short phrase like `Fix flaky checkout test`, summarised from your first prompt
by the agent you selected, running on its cheapest model in a throwaway sandbox
(see [Git checkout association and linked
worktrees](#git-checkout-association-and-linked-worktrees)). If that cannot be done,
the opening words of the prompt name it instead, as before.

`n` first goes to an interaction that newly went idle, wherever it is. If none
are waiting, it steps through every interaction still running — waiting on you
or mid-turn, seen or unseen — in list order, wrapping at the end. Stopped and
completed or abandoned interactions are skipped. `N` likewise steps through the interactions
actively working. While the live-interaction list is scoped to the current
Workspace (`w`), both stay within that Workspace.

The tag editor lists every tag already used by a Session, across Workspaces.
Type to fuzzy-filter the list (`Esc` clears the filter), move with the arrows
or `Ctrl-J`/`Ctrl-K`, and use `Space` to select or clear a tag, `Ctrl-N` to
type and add a new tag (starting from the filter), and `Enter` to save. Tags remain with the durable Session when its interaction stops or is
resumed.

## Read the session

| Key | View or action |
| --- | --- |
| `r`, `l`, `t`, `d` | raw wire records, client/server log, transcript, Workspace/interaction details; press again for events |
| `Q` | quota readings observed by the server (`R` there: keep at it after a rate limit) |
| `F` | files associated with the selected event (or the whole session) |
| `f` | highlight links in the conversation (`j`/`k` moves between them) |
| `X` | typed answer from the last turn |
| `p` / `P` | toggle side preview / full-screen preview |
| `C` | preview the newest command |
| `y` | copy the selected item to the clipboard |
| `c` | show all events instead of the conversation only (events and transcript); the border then says `all events · minor shown` or `minor hidden` |

In the event list: `j`/`k` moves the selection by line, the arrows (while no
preview is open) move it between events with details, and `J`/`K` scroll the
interaction ten text lines without changing the selection.
At the bottom, at least the final five interaction lines remain visible (or
all of them when the interaction is shorter).
`g`/`G` jumps first/last, and `Enter` on a `branch` marker opens the linked
interaction (from either side of a branch). Otherwise, `Space`/`Enter`/`o`
folds the selected event; `O` expands only it, `z R` expands all, `z M`
collapses all, and `m` hides/shows minor events. With a preview open, side or
full-screen, `↑`/`↓` scroll it ten lines and `PgUp`/`PgDn` half its height;
in the full-screen preview `j`/`k` scroll too. On a conversation line (a message, an error or a model
change), the side preview shows the file changes the agent made during
that entry's turn — the same stretch `e` lists — rather than the line itself;
the full-screen preview (`P`) still shows the entry. On a `branch` marker,
both previews show the linked interaction's conversation instead: a branch
from where its own history begins, after what it copied from this one, and a
source in full. A live interaction's log keeps growing while it is shown.
Diffs mark additions and removals with a green `+` and a red `-` and highlight
the code in the language of each file's extension, with each line's number in
the file beside it; only the changed lines are shown. Claude's edits carry no line numbers, so they are found in
the file as it is now: an edit that has since been changed again is shown
unnumbered. Raw, log, quota, and transcript
use `j`/`k` plus `g`/`G` to navigate. In the raw view, `v` switches between
Styra's captured app-server wire traffic and the provider's native persisted
session JSONL (when the provider still has it).

`f` starts link highlighting from the selected event, wrapping to the bottom-most earlier link if no later link remains. While it is on, the conversation around the links and file references is dimmed; preview content stays at normal brightness. It opens the event list when pressed from transcript or
full-screen preview. While a link is highlighted, `j` and `k` move forward and
backward through Markdown links and backticked file references (`src/app.rs:120`)
across the visible conversation, `Enter` opens the link in the configured editor (or, for an `http`/`https` address, the configured browser), `Space` opens a menu of actions to take on the link — opening it, as `Enter` does, and for a file, mounting it read-only or read-write into this interaction's sandbox (it applies when the Session next launches, and is refused while the interaction is running), and `Esc` returns `j`/`k` to normal list
navigation.

A relative file link is looked for in the directory the interaction is working
in, then in each directory above it up to the Workspace's own directory, and
last, when the Workspace is associated with a Git repository, from the top of
the checkout it is in (the interaction's own linked worktree, when it has one);
the nearest match wins. A link that names no file there is reported rather than
opened, previewed, or mounted.

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
again. The bottom border of that view says which way `R` is set. The top of it
summarises where each provider's windows stand right now and when each one
resets — a reset on a later day, such as a weekly Codex window, is shown with
its date as well as its time — and below that sit the most recent readings,
as many as the panel is tall enough for.

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
exist. A sandbox cannot gain mounts while it runs, so on an idle interaction
Styra restarts the agent under the new grant (resuming the same conversation)
before you send; on a stopped one the grant applies at the next resume. While
the agent is working, no grant is offered and Styra tells you the inserted path
is unreachable now.

## Control isolation and reusable launch policy

Press `d` before starting an interaction. The view has two layers: the
Workspace policy (shared and durable) and this interaction's additions. `Tab`
switches focused layer; `j`/`k` selects a mount.

| Key | Focused-layer change |
| --- | --- |
| `w` | cycle network permission |
| `T` | select Driva templates |
| `m` / `x` | add a mount (prefilled with the Workspace path) / remove selected mount |
| `I` | make this interaction add to, or ignore, Workspace policy |
| `U` | promote this interaction's additions into Workspace policy |
| `D` | save this interaction's additions as new-client defaults |
| `G` | set the Workspace Git checkout; submit an empty path to clear it |

In the mount prompt, `Ctrl+W` removes the final path component.

Workspace edits take effect for future launches everywhere in that Workspace.
Interaction edits apply only to its next launch/resume unless promoted. Existing
live sandboxes are immutable, with one exception: while the interaction is idle,
`m` and `x` still work (on either layer), and Styra restarts the agent under the
new mounts, resuming the same conversation. The rest of the policy stays fixed
until the interaction is stopped (`S`); its settings and keys stay on screen,
dimmed, to show they are disabled rather than gone.

## Git checkout association and linked worktrees

These are related but independent Workspace features:

| Feature | How it is enabled | What a future interaction receives |
| --- | --- | --- |
| **Git checkout association** | When the TUI creates a Workspace, it finds the nearest enclosing checkout and records its canonical root automatically. | The checkout is mounted read-only at its host path; its Git metadata/common directory is writable, so Git can operate on that checkout. |
| **Linked worktrees** | Send any message with `Ctrl+Enter` (or press `W`). | A branch and linked checkout of its own, mounted writable at `/tmp/styra/workspace`, plus the repository's shared Git metadata. |

The automatic repository association is visible in Workspace metadata. Press `d`, then
`G`, to replace it; enter any path inside the checkout and Styra stores its
root. Submit an empty path to clear it. A non-TUI client can also update it
through the local socket API:

```sh
printf '%s\n' \
  '{"operation":"set_workspace_git_repository","data":{"workspace_id":"WORKSPACE_ID","git_repository":"/path/in/checkout"}}' \
  | socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/styra/styra.sock"
```

Use `"git_repository":null` to clear the association. The checkout must exist
and be inside a Git repository.

The repository worktrees are made from is discovered from the **Workspace host
directory**, not from the optional checkout association, so the
`Ctrl+Enter` action only has an effect when that directory is inside a Git working tree.

There is nothing to ask the agent for: before an interaction starts, Styra
creates a branch named after the work and its Session
(`styra/fix-flaky-checkout-test-<SESSION-ID>`), checks it out under the
Workspace's store directory, and gives the agent that checkout as its
workspace. The readable half comes from your first prompt, summarised by the
agent you selected running on its cheapest model, in a throwaway sandbox that
holds nothing but that agent — so your own `git branch` says what each branch
is for, at a fraction of a cent. The same summary names the Session, so the
picker reads `Fix flaky checkout test` where it used to show the opening
sixty characters of your prompt — unless you named the launch yourself, in
which case your name stands. If the agent cannot be reached, or does not
answer within twenty seconds, the first prompt's own words are used for the
branch and the prompt itself still names the Session, and a launch you started
without a prompt keeps the bare Session id; naming never fails a launch. The
agent simply works where it is put and can commit freely; your
own checkout is not mounted, so nothing it does reaches the files or the branch
you have open. Resuming the Session comes back to the same checkout with its
uncommitted work intact, and the details view (`d`) names it as the
interaction's workspace, beside the Workspace's own host path.

Branching such a Session (`B`) gives the new one a checkout of its own, forked
from the branch the source is on and carrying the same topic in its name
(`styra/fix-flaky-checkout-test-<NEW-SESSION-ID>`). The two conversations
continue apart, so their working trees do too: the branch starts from what the
source had committed, and anything the source has left uncommitted stays in the
checkout it is still working in. A Session working in the Workspace directory
rather than a checkout passes that on instead — its branch works there too.

A linked checkout is always of the whole repository, so a Workspace naming a
directory below the checkout root gets the root: the agent sees more of the
tree this way, not less. Merging the branch back afterwards is yours to do, on
the host, with ordinary Git — Styra never merges or deletes a branch.

### Seeing which checkouts exist

`styra worktrees` lists every checkout Styra knows of, each followed by the
Sessions that work in it, so you can tell which conversation a directory came
from before you clean anything up:

```sh
styra worktrees         # the Workspace covering the current directory
styra worktrees --all   # every Workspace the server knows
```

Sessions launched from one another's checkout are listed together under it,
with whether each is live, active, completed, abandoned or sealed. A Session whose
checkout was cleaned up is listed under its branch alone, and a directory in
the worktree parent that no Session records is listed with no Sessions under
it. Listing changes nothing. Inside the interface, `w` shows the same list and
jumps to the session you pick.

### Cleaning up the checkouts you are done with

A worktree is a whole copy of the repository per conversation, and they add up.
`styra clean-worktrees` deletes the ones that have nothing left in them:

```sh
styra clean-worktrees         # the Workspace covering the current directory
styra clean-worktrees --all   # every Workspace the server knows
```

A checkout is deleted only when you have marked its Session complete or
abandoned (`C` or `Z` in the Session list), or it was sealed, *and* `git status` in it is clean. The
branch is never touched: the commits stay exactly where you expect to find
them in `git branch`, and the Session goes on recording that branch with no
directory beside it. Resuming such a Session checks the branch out again, in
the same place and under the same name, so cleaning up costs you nothing but
the disk space.

Everything the pass considered is printed, including what it left alone and
why — a checkout with uncommitted changes, or one a live interaction is still
working in, is reported and kept. Nothing else prunes these checkouts; they go
when you say so.

`n` in a Session that has a checkout starts the next one **in that same
checkout**, on its branch and among its uncommitted work, rather than back in
the Workspace directory — so a second agent, or a fresh conversation about the
same half-finished change, needs no new branch and no copying. The blank screen
stands in the checkout and the footer names it. Send that first prompt with
`Ctrl+Enter` instead to say you wanted a branch of its own after all, and with
`W` afterwards to give a Session that started in the Workspace directory a
checkout later. Nothing is duplicated: both Sessions write to one working tree,
which is the point when they are meant to collaborate and worth knowing when
they are not.

## What Styra records and does not do

Each Session stores metadata, a raw JSONL journal, and diagnostics under its
Workspace. The UI can be reopened or multiple local clients can observe the
same Session. Styra is local-only (Unix socket, no TCP listener), does not
review or submit work, and does not recreate lost provider context; it relies
on the provider's native resume support.
