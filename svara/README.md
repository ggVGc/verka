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
| `lua/svara/core.lua` | What the commands do: `send_message`, and `start` behind `:Svara`. |
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
contract })`, `styra:resume_session(id, { selection, launch })`,
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
`styra:set_auto_retry(id, enabled)`.

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
already see — `interaction.selection`, `summary.selection`.

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
:Svara Why does resuming a branched session lose its tags?
```

`:Svara` starts a new interaction with the rest of the line as its first
prompt, in the Workspace covering Neovim's working directory — `:Svara` asks
the server which one that is, so there is no id to look up and nothing to
configure per project. The prompt goes out with the file and line being viewed
in front of it — `I am viewing /path/to/file.lua:42` — because a prompt typed
in an editor is nearly always about what is on screen, and saying so beats
typing the path. A buffer with no file behind it adds nothing.

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
so rather than guessing, for the reason in [Selections](#selections).

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
nvim --headless -u NONE -l tests/core_spec.lua   # send_message over a socket
nvim --headless -u NONE -l tests/api_spec.lua    # the API, on a host of its own
nvim --headless -u NONE -l tests/nvim_spec.lua   # the Neovim host, for real
```
