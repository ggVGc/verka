# Svara

Svara is Styra from inside Neovim: a Lua client for `styra-server`, speaking
the generated protocol library and adding the pieces an editor needs — a JSON
codec, a Unix socket, and a timer to watch a turn with. The Neovim command and
the command-line program are both written against it.

| | |
|---|---|
| `lua/svara/api.lua` | **The API.** One function per interaction with the server. Mentions Neovim nowhere. |
| `lua/svara/nvim.lua` | Neovim, as the six fields the API asks a *host* for. |
| `lua/svara/protocol.lua` | Where the generated `styra.protocol` is found. |
| `lua/svara/core.lua` | What the commands do: sending to a selected interaction, `start` behind `:SvaraNew`, and `info` behind `:SvaraInfo`. |
| `lua/svara/compose.lua` | The floating message box `:SvaraNew` takes its prompt and model in. |
| `lua/svara/pending.lua` | The corner float listing the questions still waiting on an answer. |
| `../styra/protocol/lua/styra/protocol.lua` | **The vocabulary**, generated, living where it is generated from. |

## The vocabulary, which is not here

`styra.protocol` is generated from the Serde type definitions in Rust and lives
beside them, in `../styra/protocol/lua`. A copy vendored here would be a second
home for the protocol, and a second home is where drift starts. Regenerate it
with

```sh
cargo run -p protocol --bin styra-codegen -- lua
```

Lua has no manifest to declare that dependency in, so the dependency is a
search path, and `svara.protocol` is where Svara sets it: it takes the module
if something already put it on `package.path` or Neovim's runtimepath, then
`$STYRA_PROTOCOL_LUA`, then the sibling checkout beside this repository — so a
clone runs without being installed, and an installed plugin works by having the
generated file somewhere on the runtimepath.

## The API

```lua
local svara = require("svara")
local styra = assert(svara.open())          -- or { socket = "/path/to/styra.sock" }

for _, interaction in ipairs(assert(styra:interactions())) do
  print(interaction.id, svara.given(interaction.last_message) or "")
end

assert(styra:send_message("styra-7", "review this buffer"))
```

`open` connects nothing, which is why it is not called `connect`: a connection
carries exactly one request and one response, so every exchange is a whole
connection. It takes `socket` (defaulting to `$STYRA_SOCKET`, then
`$XDG_RUNTIME_DIR/styra/styra.sock`, the socket `styractl` uses), `timeout` in
milliseconds, and `host`.

Every function returns the response's data, or `nil` and a message — the
server's own error, or the reason the request never left the editor. A request
the server would refuse is refused here, on the line that built it, rather than
by the server a round trip later.

**The server.** `styra:health()`, `styra:shutdown()`.

**Workspaces.** `styra:workspaces()`, `styra:workspace(id)`,
`styra:workspace_for_path(directory)` — the Workspace covering an absolute
directory, which may be anywhere beneath the Workspace's own; a directory no
Workspace covers comes back as `nil` and why, like any other absence —
`styra:create_workspace(host_path, { name, git_repository })`,
`styra:set_workspace_git_repository(workspace_id, path)` — a `nil` path
disassociates the Git checkout, and is sent as an explicit null —
`styra:set_workspace_worktrees_enabled(workspace_id, enabled)`.

**Sessions.** `styra:sessions(workspace_id)`, `styra:session(id, { raw })`,
`styra:create_session(workspace_id, selection, { launch, message, name,
contract, focus })` — `focus` asks a Styra showing that Workspace to switch
to the new interaction, as `:SvaraNew` does — `styra:resume_session(id, { selection, launch })`,
`styra:rename_session(id, name)` — `nil` clears it —
`styra:branch_session(id, { at_ms, history, provider })`,
`styra:convert_session_provider(id)`, `styra:provider_raw(id)`,
`styra:shell(id)`.

**Live interactions.** `styra:interactions()`, `styra:load_interaction(id)`,
`styra:updates(id, after, { raw })`, `styra:send_message(id, text, { contract,
selection })`, `styra:queue_message(id, text, { contract })`,
`styra:send_queued_message(id)`, `styra:clear_queued_messages(id)`,
`styra:interrupt(id)`, `styra:stop(id)`, `styra:close(id)`,
`styra:set_selection(id, selection)`, `styra:set_working_directory(id, dir)`,
`styra:set_auto_retry(id, enabled)`, `styra:set_completed(id, state)`, where
`state` is `"active"`, `"completed"`, `"abandoned"` or `"sealed"`, and
`"sealed"` is permanent.

**Answers.** `styra:answer(id, { contract })` parses the last agent message
under a contract. A reply that missed its contract is an `Answer` too, not an
error in place of one, so `svara.answered(answer)` returns the value or `nil`
and why — and `answer.source` still carries what the agent actually said.

Not here: the Workspace sandbox policy (`workspace_launch`,
`change_workspace_launch`, `list_templates`, `plan_session`) and the quota log.
Those are Driva's and the account's business rather than an editor's, and are
left to `styractl` until an editor has a use for them. `styra:call(request,
expected)` sends any request in `protocol.request` at all, so neither is walled
off.

### Selections

A selection is a table or a profile name, `provider:model/effort`:

```lua
styra:create_session(workspace.id, "claude:claude-opus-5/xhigh", {
  message = "read src/auth.rs and tell me what it trusts",
})
```

The shorter forms Styra's command line accepts (`claude`, `claude/high`) are
not accepted here, deliberately: they mean "this provider's declared defaults",
those defaults live in the server's Rust and never appear on the wire, and a
copy of them in this plugin would drift silently the first time one changed. A
caller holding only a provider takes the model and effort from a session it can
already see — `interaction.selection`, `summary.selection` — or asks the
server for the catalog:

```lua
for _, summary in ipairs(assert(styra:models())) do
  print(summary.provider, summary.model, table.concat(summary.efforts, " "))
end
```

`styra:models()` is every model a session can be launched on, provider by
provider and most capable first, each with the reasoning-effort rungs that
model accepts (`efforts`, lowest first, empty for a model that takes none) and
the rung a launch takes when nothing names one (`default_effort`). It is the
catalog the Styra TUI's launcher is built from, asked for rather than copied
for the same reason the shorthands are refused.

### Watching a turn

A turn takes minutes and a connection carries one request, so a client watching
one polls. `follow` does that on a timer, off the editor's main thread of
attention:

```lua
local watching = styra:follow("styra-7", { interval = 250 }, function(update)
  if update.type == "event" and update.data.type == "agent_message" then
    vim.notify(update.data.text)
  end
end)

watching.stop()
```

`ask` is the whole of one typed question, which is three requests because that
is the protocol's own shape: `send_message` names the contract, `updates` is
polled until the turn completes, and `turn_answer` parses the reply the server
already has. Nothing blocks; the answer arrives in the callback.

```lua
styra:ask("styra-7", "which files handle auth?", { contract = "files" },
  function(files, answer, err)
    if err then
      return vim.notify("Svara: " .. err, vim.log.levels.ERROR)
    end
    for _, location in ipairs(files) do
      print(location.path, svara.given(location.line) or "")
    end
  end)
```

The sequence it polls from is read *before* the message goes out, so a turn
that finished before the question cannot be mistaken for its answer.

`styra:await_answer(id, { after, contract }, on_answer)` is the second half
of `ask` on its own: it waits for the turn under way to complete, then reads
the answer. It is for a turn that was not sent with `send_message`, such as
the first turn of a Session that `create_session` launched with a `message` and
a `contract`. `after` must be a sequence from before that turn began.

## The host

Everything editor-specific is a *host*: a table of six fields — `encode`,
`decode`, `null`, `getenv`, `exchange`, `defer` — implemented for Neovim in
`lua/svara/nvim.lua` and passed to `open` as `host`. The API itself mentions
Neovim nowhere, so a test needs no editor, no socket and no server:

```lua
local styra = assert(require("svara.api").open({ host = fake }))
```

`exchange(path, line, timeout)` blocks, which is what one short round trip
wants; `defer(milliseconds, fn)` is what keeps a minutes-long turn from
blocking with it.

## Neovim

Add this directory to Neovim's runtime path with your plugin manager, then run:

```vim
:SvaraNew
```

Run `:Svara` with no argument to choose from the active interactions in the
Workspace covering the currently viewed file. The choice is remembered per
Workspace until Neovim exits. Thereafter `:Svara Review this buffer` sends to
that interaction. `:SvaraNew` starts a new interaction. Both commands use the Workspace covering the viewed
file, rather than Neovim's working directory.

`:SvaraNew` opens Styra's message box as a floating window: ` message ` at the
top left of its border, the model and effort it will start on at the top right,
and "Enter to send · Ctrl+Enter to send in a new Git workspace" until something
is typed. It is centred, at most 80 columns wide, and grows with the prompt
for as long as the screen has room, so all of it stays in sight. It is an
ordinary buffer, in Insert mode to begin with, so the whole of Vim is there to
write it with, and it is driven the way Styra's box is:

| Key | Mode | Does |
|---|---|---|
| `Enter` | Normal | send, starting in this Workspace |
| `Ctrl+Enter` | Normal or Insert | send, starting in a new Git workspace and branch — a linked checkout of its own |
| `Ctrl+L` | Normal or Insert | choose the model |
| `Enter` | Insert | a newline, as anywhere else |
| `Esc` or `q` | Normal | close without sending |

`Esc` in Insert mode only leaves Insert mode, as anywhere else. Closing the
window any other way — `:close`, moving to another window — sends nothing too.
Anything after `:SvaraNew` on the command line is where the prompt starts.
`Ctrl+Enter` needs a terminal that tells it apart from `Enter`, as it does in
Styra.

Sending does not hold the editor while the server works. The box stays open,
read-only, with a spinner, what is happening — `starting…`, or `creating a Git
workspace and branch…`, which takes a few seconds — and how long it has taken
in its bottom border. It closes once the interaction has started. If the start
fails, the prompt is handed back to be edited and sent again, with the reason
under it. Closing the box while it waits only stops the showing: the start
goes on, and says when it has finished.

The model named in the border is the one the rules below give. `Ctrl+L` opens
the model picker over the box, with `vim.ui.select` — so in Telescope, fzf-lua
or whatever else `vim.ui` is configured with — as one list: that model at the
top, said with where it came from, then every model the server offers, then
"another model…" for typing a profile name out with `vim.ui.input` — a catalog
is not a closed set, and an id newer than the server's tables is still
launchable. Choosing the first entry keeps it and stores nothing, leaving the
rules in charge. Anything else asks for a reasoning effort next, from the rungs
that model accepts, and is remembered in `vim.g.svara_selection`, so it is
chosen once for a stretch of work rather than at every `:SvaraNew`. The box
stays open under the picker, and once it closes — chosen or backed out of —
goes back to the prompt as it was left, naming the model chosen.

When there is nothing to name — no `vim.g.svara_selection` and no Session in
the Workspace — the border says
`no model · Ctrl+L`, and sending opens the picker first and sends once a model
is chosen.

The prompt goes out with the file, line and column being viewed after it —
`Source: /path/to/file.lua:42:7` — because a prompt typed in an editor is nearly always
about what is on screen and saying so beats typing the path. A buffer with no
file behind it cannot select, send, or start an interaction.

`:SvaraInfo` says what the other commands would do here, which they otherwise
decide silently:

```
directory  /home/me/verka/styra/protocol/src
server     styra-server at /run/user/1000/styra/styra.sock
workspace  inner — /home/me/verka/styra/protocol (1790683094903-5393-1)
git        /home/me/verka
sessions   2 stored
model      claude:claude-opus-5/high, from the newest Session in the Workspace
selected   styra-7 (running)
live       1 interaction can take a message
```

Each line is filled in as far as the one above it allows: without a server
there is no Workspace to find, and without a Workspace no interactions and no
model, so an unanswerable question says why in place of its value rather than
taking the whole command down with it. The Workspace shown is the one covering
the viewed file — the innermost one, when Workspaces nest — so this is also
the answer to "why did that go somewhere I did not expect". Unlike the other
commands, a buffer with no file behind it is not refused: `:SvaraInfo` answers
for Neovim's working directory and says that is what it did.

The same thing from Lua, where `info` is the table the lines are made of —
`workspace`, `selection`, `selection_source`, `selected_interaction`,
`interactions`, and an `*_error` beside anything missing:

```lua
local info = assert(require("svara").info({ directory = vim.fn.expand("%:p:h") }))
print(table.concat(require("svara").info_lines(info), "\n"))
```

The same thing from Lua, where the directory, the name and the answer's
contract can all be said, and the prompt is sent as written:

```lua
local session, err = require("svara").start("what does this module trust?", {
  directory = vim.fn.expand("%:p:h"),
})
```

The model is `vim.g.svara_selection`, a profile name:

```lua
vim.g.svara_selection = "claude:claude-opus-5/xhigh"
```

With none set, the new interaction runs under the newest Session in that
Workspace — a Workspace being worked in has already been launched under
something. A Workspace with no Session yet and no `vim.g.svara_selection` says
so rather than guessing, for the reason in [Selections](#selections). From
Lua the rules are used as they stand — the asking is `:SvaraNew`'s, and
`start` takes an explicit `selection` when the caller has already chosen. The
three parts of that question are `svara.core.selection_for_directory`, which
answers what would be used and where it came from,
`svara.core.available_models`, which is the server's catalog, and
`svara.core.remember_selection`, which validates a profile name and stores it
in `vim.g.svara_selection`.

`:SvaraAsk` asks a question whose answer is places in the code, and puts
them in the quickfix list:

```vim
:SvaraAsk where is the session token checked?
```

Every question starts a new interaction of its own in the Workspace covering
the viewed file, with the question as its first turn under the `files`
contract, followed by the file, line and column being viewed, as `:Svara`
sends it. So it starts from nothing but itself, never lands in the middle of
a turn, and leaves the interaction selected with `:Svara` as it was. It runs
on the model `:SvaraNew` would use; with none to take, the same picker comes
first. Styra is not switched to it. Once it has answered, it is marked
completed, which moves it out of the way in Styra's listing while its history
stays readable. A reply that missed its contract still counts as an answer.
An interaction that failed or ended without answering stays active, so you can
look into it.

With no question on the command line, `:SvaraAsk` opens the same box as
`:SvaraNew`, titled ` question `. The model is named in its border, `Ctrl+L`
changes it, and `Enter` asks. It has no `Ctrl+Enter`, because a question needs
no Git workspace of its own. The box spins with `asking…` until the
interaction is up, then closes and leaves the wait to the float. If the
interaction cannot start, the question is handed back to be edited, as in
`:SvaraNew`.

Nothing waits while the agent works. A float at the top right lists every
question still under way, with a spinner and how long it has taken. It never
takes the focus, and it closes once the last answer is in. Then the quickfix
list is replaced with the locations, titled with the question, and opened.
Paths the agent gave relative to its root are looked for in the directory
the interaction runs in, which is its own Git workspace when it has one, and
then in the Workspace's directory. A reply that named no locations leaves the
quickfix list alone and says why; what the agent wrote instead is in Styra.

From Lua, `require("svara").find(question, { directory, selection }, on_done)`
does the same without the float or the quickfix list. It returns once the
interaction is up, with the handle following it and its `SessionInfo`, and calls
`on_done(items, nil, answer)` with items ready for `setqflist`, or
`on_done(nil, error, answer)`.

To send to a session that is already live, name it:

```vim
:SvaraSend styra-7 Review this buffer
```

The first argument is the ID of an existing live session. Everything after it
is sent as the message. The same thing from Lua:

```lua
local sent, err = require("svara").send_message("styra-7", "Review this buffer")
```

## CLI

The CLI is a Lua script executed by headless Neovim, so it has the same libuv
Unix-socket and JSON implementation as the plugin and needs no extra Lua
packages:

```sh
./bin/svara styra-7 "Review this buffer"
```

By default Svara connects to `$XDG_RUNTIME_DIR/styra/styra.sock`, matching
`styra-server`. Set `STYRA_SOCKET` to use another socket. Neovim 0.10 or newer
is required.

## Test

```sh
nvim --headless -u NONE -l tests/core_spec.lua    # send_message over a socket
nvim --headless -u NONE -l tests/api_spec.lua     # the API, on a host of its own
nvim --headless -u NONE -l tests/nvim_spec.lua    # the Neovim host, for real
nvim --headless -u NONE -l tests/info_spec.lua    # what :SvaraInfo answers
nvim --headless -u NONE -l tests/picker_spec.lua  # the model list :SvaraNew offers
nvim --headless -u NONE -l tests/compose_spec.lua # the window its prompt and model are chosen in
nvim --headless -u NONE -l tests/find_spec.lua    # what :SvaraAsk asks, the quickfix items, the float
```
