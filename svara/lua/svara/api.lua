-- Styra, as functions a Neovim plugin can call.
--
-- The generated `styra.protocol` is the wire vocabulary and nothing else: it
-- builds and checks request tables, and has no opinion about how those tables
-- become bytes or how the bytes reach the server. This module is the other
-- half — one function per interaction the editor can have with Styra, each
-- returning the response's data or `nil` and a message:
--
--   local styra = assert(require("svara.api").open())
--   local workspaces = assert(styra:workspaces())
--   assert(styra:send_message("styra-7", "review this buffer"))
--
-- Every operation name, field name and enum spelling below comes from
-- `styra.protocol`, so nothing here restates the wire format and a request
-- this file gets wrong is refused on the line that built it rather than by the
-- server a round trip later.
--
-- Nothing in here touches Neovim. The editor-specific pieces — the JSON codec,
-- the Unix socket, the timer a long turn is polled with — are a *host*, passed
-- in at `open` and defaulting to `svara.nvim`; see that file for the six
-- fields a host has. A test hands `open` its own host and needs no server, no
-- socket and no editor:
--
--   local styra = assert(api.open({ host = fake_host }))
--
-- What is missing is missing on purpose: the Workspace sandbox policy
-- (`workspace_launch`, `change_workspace_launch`, `list_templates`,
-- `plan_session`) and the quota log are Driva's and the account's business,
-- not an editor's, and are left to `styractl` until an editor has a use for
-- them. `protocol.request` still has them, and `Client:call` below sends any
-- request at all, so neither is walled off.

local protocol = require("svara.protocol")

local M = {}

--- The vocabulary this module speaks, for a caller wanting the raw tables.
M.protocol = protocol

local DEFAULT_TIMEOUT = 30000
local DEFAULT_INTERVAL = 250

-- Values
-- ------

--- The value of a nullable field, as an absence rather than a sentinel.
---
--- A field the server sent as null decodes to a *value* in Lua, not to `nil`,
--- so anything read for display goes through here first.
function M.given(value)
  if value == nil or value == protocol.null then
    return nil
  end
  return value
end

local function one_of(kind, value, what)
  if type(value) ~= "string" then
    return nil, string.format("%s must be named with a string", what)
  end
  for _, name in ipairs(protocol.enums[kind]) do
    if name == value then
      return value
    end
  end
  return nil,
    string.format(
      "%q is not %s; Styra knows %s",
      value,
      what,
      table.concat(protocol.enums[kind], ", ")
    )
end

--- A `Contract`, from its wire spelling: "text", "lines", "files" or "json".
function M.contract(value)
  return one_of("Contract", value, "a contract")
end

--- A `Provider`, from its wire spelling: "codex", "codex-exec" or "claude".
function M.provider(value)
  return one_of("Provider", value, "a provider")
end

--- An `Effort`, from its wire spelling: "minimal" through "max".
function M.effort(value)
  return one_of("Effort", value, "a reasoning effort")
end

--- A `Selection`, from a table or from a profile name.
---
--- A table is `{ provider = ..., model = ..., effort = ... }` and is checked
--- field by field. A string is a profile name, `provider:model/effort`, the
--- same one a journal records and a status line shows:
---
---   api.selection("claude:claude-opus-5/xhigh")
---
--- The shorter forms Styra's own command line accepts (`claude`,
--- `claude/high`) are *not* accepted here, and that is deliberate rather than
--- missing: they mean "this provider's declared defaults", and those defaults
--- live in the server's Rust, are not on the wire, and cannot be read from
--- here. A copy of them in this file would be a second home for them, drifting
--- silently the first time one changed. So a selection is said in full, and a
--- caller with only a provider in hand takes the model and effort from a
--- session it can see — `interaction.selection`, `summary.selection`.
function M.selection(value)
  if type(value) == "table" then
    local ok, err = protocol.validate("Selection", value)
    if not ok then
      return nil, "selection " .. err
    end
    return value
  end
  if type(value) ~= "string" then
    return nil, "a selection must be a table or a profile name"
  end

  local head, effort = value:match("^([^/]*)/(.*)$")
  if not head then
    return nil,
      string.format(
        "the profile %q names no reasoning effort; say it in full, as in "
          .. "\"claude:claude-opus-5/xhigh\"",
        value
      )
  end
  local provider, model = head:match("^([^:]*):(.*)$")
  if not provider or model == "" then
    return nil,
      string.format(
        "the profile %q names no model; say it in full, as in "
          .. "\"claude:claude-opus-5/xhigh\"",
        value
      )
  end

  local named, provider_error = M.provider(provider)
  if not named then
    return nil, provider_error
  end
  local spent, effort_error = M.effort(effort)
  if not spent then
    return nil, effort_error
  end
  return { provider = named, model = model, effort = spent }
end

--- The profile name of a selection: `provider:model/effort`.
function M.selection_name(selection)
  return string.format("%s:%s/%s", selection.provider, selection.model, selection.effort)
end

-- Arguments
-- ---------

local function text_argument(value, what)
  if type(value) ~= "string" or value == "" then
    return nil, what .. " must be a non-empty string"
  end
  return value
end

local function flag_argument(value, what)
  if type(value) ~= "boolean" then
    return nil, what .. " must be true or false"
  end
  return value
end

-- Clients
-- -------

local Client = {}
Client.__index = Client

--- The socket `styractl` uses, unless told otherwise.
local function default_socket(host)
  local configured = host.getenv("STYRA_SOCKET")
  if configured and configured ~= "" then
    return configured
  end
  local runtime = host.getenv("XDG_RUNTIME_DIR")
  if not runtime or runtime == "" then
    return nil,
      "XDG_RUNTIME_DIR is not set; pass options.socket or set STYRA_SOCKET"
  end
  return runtime .. "/styra/styra.sock"
end

--- Open a client.
---
--- Nothing is connected here, which is why this is not called `connect`: a
--- connection carries exactly one request and one response, so every exchange
--- is a whole connection. What `open` returns is where to reach the server and
--- how to encode for it.
---
--- `options.socket` defaults to `$STYRA_SOCKET`, then to
--- `$XDG_RUNTIME_DIR/styra/styra.sock`, the socket `styractl` uses.
--- `options.timeout` is the milliseconds one exchange may take, 30000 by
--- default. `options.host` replaces Neovim with anything implementing the same
--- six fields — see `svara.nvim`.
---@param options? { socket?: string, timeout?: integer, host?: table }
---@return table? client
---@return string? error
function M.open(options)
  options = options or {}
  local host = options.host or require("svara.nvim")

  local socket = options.socket
  if not socket or socket == "" then
    local found, err = default_socket(host)
    if not found then
      return nil, err
    end
    socket = found
  end

  return setmetatable({
    socket = socket,
    timeout = options.timeout or DEFAULT_TIMEOUT,
    host = host,
    protocol = protocol,
    given = M.given,
  }, Client)
end

--- Build one request, returning `nil` and the message instead of raising.
---
--- `protocol.build` raises on a request the server would refuse, which is the
--- right answer for a script and the wrong one inside an editor, where the
--- traceback lands in a notification. The check is the same; only its delivery
--- changes.
function Client:build(operation, data)
  -- Lua cannot tell a key absent from one set to nil, so a null is a
  -- sentinel, and which sentinel it is belongs to the host's codec.
  protocol.use_null(self.host.null)
  local built, result = pcall(protocol.build, operation, data)
  if not built then
    -- The raise names the file and line it came from, which is this file and
    -- of no use to whoever is looking at the notification.
    return nil, (tostring(result):gsub("^.-:%d+: ", ""))
  end
  return result
end

--- Send one request table and return the decoded reply, whatever it says.
function Client:request(request)
  local encoded, line = pcall(self.host.encode, request)
  if not encoded then
    return nil, "could not encode the request: " .. tostring(line)
  end

  local reply, transport_error = self.host.exchange(self.socket, line, self.timeout)
  if not reply then
    return nil, transport_error
  end

  local decoded, wire = pcall(self.host.decode, reply)
  if not decoded or type(wire) ~= "table" then
    return nil, "Styra returned a reply that is not JSON: " .. reply
  end
  return wire
end

--- Send one request and return the data of the response it must answer with,
--- or `nil` and a message.
---
--- The expected response is named by the caller so a surprise comes back as a
--- message rather than a missing field three lines later. Every function below
--- is this call with its arguments arranged.
---
---   styra:call(protocol.request.health(), protocol.Response.HEALTH)
function Client:call(request, expected)
  local reply, err = self:request(request)
  if not reply then
    return nil, err
  end
  return protocol.expect(reply, expected)
end

--- Build, send, and read back in one step: what every operation below does.
local function operate(client, operation, data, expected)
  local request, err = client:build(operation, data)
  if not request then
    return nil, err
  end
  return client:call(request, expected)
end

-- The server
-- ----------

--- Check that the server is reachable, returning its `Health`.
function Client:health()
  return operate(self, "health", nil, protocol.Response.HEALTH)
end

--- Ask the server to remove its socket and exit. Any live interactions it owns
--- die with it, so this is the deliberate counterpart to the daemon outliving
--- its clients.
function Client:shutdown()
  return operate(self, "shutdown", nil, protocol.Response.ACCEPTED)
end

-- Workspaces
-- ----------

--- Every Workspace, as `WorkspaceSummary` tables.
function Client:workspaces()
  return operate(self, "list_workspaces", nil, protocol.Response.WORKSPACES)
end

--- One Workspace by id.
function Client:workspace(id)
  local named, err = text_argument(id, "the Workspace id")
  if not named then
    return nil, err
  end
  return operate(self, "workspace", { id = named }, protocol.Response.WORKSPACE)
end

--- The Workspace a directory belongs to.
---
--- The directory may be the Workspace's own or anywhere beneath it, which is
--- what makes this answerable from a working directory — the question an
--- editor actually has. The innermost Workspace wins when they nest.
---
--- A directory no Workspace covers is an absence with a reason, returned the
--- same way as a failure: from inside the editor both end as the same
--- notification, and a caller wanting to tell them apart has `workspaces()`.
---
--- The path must be absolute. The server resolves it in its own process,
--- where a relative path would mean a directory under the daemon rather than
--- under the editor.
function Client:workspace_for_path(path)
  local directory, err = text_argument(path, "the directory")
  if not directory then
    return nil, err
  end
  if directory:sub(1, 1) ~= "/" then
    return nil, directory .. " must be an absolute path"
  end
  local found, failure = operate(
    self,
    "workspace_for_path",
    { path = directory },
    protocol.Response.WORKSPACE_FOR_PATH
  )
  if not found then
    return nil, failure
  end
  found = M.given(found)
  if not found then
    return nil, "no Styra Workspace covers " .. directory
  end
  return found
end

--- Create a Workspace over a host directory.
---@param host_path string
---@param options? { name?: string, git_repository?: string }
function Client:create_workspace(host_path, options)
  local path, err = text_argument(host_path, "the Workspace host path")
  if not path then
    return nil, err
  end
  options = options or {}
  return operate(self, "create_workspace", {
    host_path = path,
    name = options.name,
    git_repository = options.git_repository,
  }, protocol.Response.WORKSPACE_CREATED)
end

--- Associate a Workspace with a Git checkout, or with none.
---
--- The path may be anywhere inside the checkout; the server stores its root.
--- A `nil` path disassociates the Workspace, which is a thing to say rather
--- than a thing left unsaid, so it is sent as an explicit null.
function Client:set_workspace_git_repository(workspace_id, git_repository)
  local id, err = text_argument(workspace_id, "the Workspace id")
  if not id then
    return nil, err
  end
  return operate(self, "set_workspace_git_repository", {
    workspace_id = id,
    git_repository = git_repository or self.host.null,
  }, protocol.Response.WORKSPACE_GIT_REPOSITORY_UPDATED)
end

--- Opt this Workspace's launches in or out of linked-worktree creation.
function Client:set_workspace_worktrees_enabled(workspace_id, enabled)
  local id, err = text_argument(workspace_id, "the Workspace id")
  if not id then
    return nil, err
  end
  local flag, flag_error = flag_argument(enabled, "worktrees_enabled")
  if flag == nil then
    return nil, flag_error
  end
  return operate(self, "set_workspace_worktrees_enabled", {
    workspace_id = id,
    enabled = flag,
  }, protocol.Response.WORKSPACE_WORKTREES_UPDATED)
end

-- Sessions
-- --------

--- Every stored Session in a Workspace, as `SessionSummary` tables.
function Client:sessions(workspace_id)
  local id, err = text_argument(workspace_id, "the Workspace id")
  if not id then
    return nil, err
  end
  return operate(self, "list_sessions", { workspace_id = id }, protocol.Response.STORED_SESSIONS)
end

--- One stored Session: its summary, its events, and — with `raw` — the
--- verbatim wire lines behind them.
---@param id string
---@param options? { raw?: boolean }
function Client:session(id, options)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  options = options or {}
  return operate(
    self,
    "stored_session",
    { id = named, raw = options.raw },
    protocol.Response.STORED_SESSION
  )
end

--- The provider's own resumable session JSONL for this Session, verbatim.
function Client:provider_raw(id)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  return operate(self, "provider_raw", { id = named }, protocol.Response.PROVIDER_RAW)
end

--- The host-side tmux endpoint for a live session's sandbox shell.
function Client:shell(id)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  return operate(self, "shell", { id = named }, protocol.Response.SHELL)
end

--- Launch a new Session in a Workspace.
---
--- The selection is a profile name or a table; see `M.selection`. `message`
--- is the first turn, sent as the session comes up, and `contract` is the
--- shape that turn's answer is asked to come back in.
---@param workspace_id string
---@param selection string|table
---@param options? { launch?: table, message?: string, name?: string, contract?: string }
function Client:create_session(workspace_id, selection, options)
  local id, err = text_argument(workspace_id, "the Workspace id")
  if not id then
    return nil, err
  end
  local picked, selection_error = M.selection(selection)
  if not picked then
    return nil, selection_error
  end
  options = options or {}
  local contract, contract_error
  if options.contract then
    contract, contract_error = M.contract(options.contract)
    if not contract then
      return nil, contract_error
    end
  end
  return operate(self, "create_session", {
    workspace_id = id,
    selection = picked,
    launch = options.launch,
    message = options.message,
    name = options.name,
    contract = contract,
  }, protocol.Response.SESSION_CREATED)
end

--- Bring a stored Session back up, optionally under another model.
---@param id string
---@param options? { selection?: string|table, launch?: table }
function Client:resume_session(id, options)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  options = options or {}
  local picked, selection_error
  if options.selection then
    picked, selection_error = M.selection(options.selection)
    if not picked then
      return nil, selection_error
    end
  end
  return operate(self, "resume_session", {
    id = named,
    selection = picked,
    launch = options.launch,
  }, protocol.Response.SESSION_RESUMED)
end

--- Name a Session, or take its name away. A `nil` name clears it.
function Client:rename_session(id, name)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  return operate(
    self,
    "rename_session",
    { id = named, name = name or self.host.null },
    protocol.Response.SESSION_RENAMED
  )
end

--- Branch a stored Session's provider transcript into a new sibling Session.
---
--- `at_ms` cuts the history at a point in time and `history` chooses what to
--- keep at it — "through_selected" (the default, a prefix) or
--- "selected_only". `provider` branches into the other agent. The source
--- Session is left untouched.
---@param id string
---@param options? { at_ms?: integer, history?: string, provider?: string }
function Client:branch_session(id, options)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  options = options or {}
  local history, history_error
  if options.history then
    history, history_error = one_of("BranchHistory", options.history, "a branch history")
    if not history then
      return nil, history_error
    end
  end
  local provider, provider_error
  if options.provider then
    provider, provider_error = M.provider(options.provider)
    if not provider then
      return nil, provider_error
    end
  end
  return operate(self, "branch_session", {
    id = named,
    at_ms = options.at_ms,
    history = history,
    provider = provider,
  }, protocol.Response.SESSION_BRANCHED)
end

--- Convert a stored Session's transcript to the other interactive provider's
--- format, as a new sibling Session ready to resume under it.
function Client:convert_session_provider(id)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  return operate(
    self,
    "convert_session_provider",
    { id = named },
    protocol.Response.SESSION_CONVERTED
  )
end

-- Live interactions
-- -----------------

--- Every interaction this server is currently running.
function Client:interactions()
  return operate(self, "list_interactions", nil, protocol.Response.INTERACTIONS)
end

--- Everything needed to make one live interaction current: its summary, its
--- updates from the beginning, and its queued messages.
function Client:load_interaction(id)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  return operate(
    self,
    "load_interaction",
    { id = named },
    protocol.Response.INTERACTION_LOADED
  )
end

--- The interaction's updates after sequence `after`, and the `next` sequence
--- to ask from.
---
--- Raw wire lines are the bulk of a long interaction's volume, so they are
--- left out unless `options.raw` asks for them.
---@param id string
---@param after? integer
---@param options? { raw?: boolean }
function Client:updates(id, after, options)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  options = options or {}
  return operate(
    self,
    "updates",
    { id = named, after = after or 0, raw = options.raw },
    protocol.Response.UPDATES
  )
end

--- Send a message to a live interaction.
---
--- `contract` asks this turn's answer to come back in a shape; read it
--- afterwards with `Client:answer`, or let `Client:ask` do both.
---@param id string
---@param text string
---@param options? { contract?: string, selection?: string|table }
function Client:send_message(id, text, options)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  local said, text_error = text_argument(text, "the message")
  if not said then
    return nil, text_error
  end
  options = options or {}
  local message, message_error = self:message(said, options)
  if not message then
    return nil, message_error
  end
  return operate(
    self,
    "send_message",
    { id = named, message = message },
    protocol.Response.ACCEPTED
  )
end

--- The `SendMessage` a text and its options make, checked.
function Client:message(text, options)
  local message = { text = text }
  if options.contract then
    local contract, err = M.contract(options.contract)
    if not contract then
      return nil, err
    end
    message.contract = contract
  end
  if options.selection then
    local picked, err = M.selection(options.selection)
    if not picked then
      return nil, err
    end
    message.selection = picked
  end
  return message
end

--- Persist a message in the session's durable queue without sending it, so it
--- survives the editor closing before the agent is idle enough to take it.
--- Returns the queue as it now stands.
---@param id string
---@param text string
---@param options? { contract?: string, selection?: string|table }
function Client:queue_message(id, text, options)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  local said, text_error = text_argument(text, "the message")
  if not said then
    return nil, text_error
  end
  local message, message_error = self:message(said, options or {})
  if not message then
    return nil, message_error
  end
  return operate(
    self,
    "queue_message",
    { id = named, message = message },
    protocol.Response.QUEUED_MESSAGES
  )
end

--- Send and remove the oldest queued message.
---
--- One wire tuple, two answers: `sent` is the message that went out, absent if
--- the queue was empty, and `queued` is what is left behind it.
---@return { sent?: table, queued: table }? result
---@return string? error
function Client:send_queued_message(id)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  local pair, call_error =
    operate(self, "send_queued_message", { id = named }, protocol.Response.SENT_QUEUED_MESSAGE)
  if not pair then
    return nil, call_error
  end
  return { sent = M.given(pair[1]), queued = pair[2] }
end

--- Discard the session's queued messages, returning how many were dropped.
function Client:clear_queued_messages(id)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  return operate(self, "clear_queued_messages", { id = named }, protocol.Response.QUEUED)
end

--- Interrupt the turn the agent is in the middle of, leaving it running.
function Client:interrupt(id)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  return operate(self, "interrupt_interaction", { id = named }, protocol.Response.ACCEPTED)
end

--- Stop the interaction.
function Client:stop(id)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  return operate(self, "stop_interaction", { id = named }, protocol.Response.ACCEPTED)
end

--- Stop the interaction and drop the server's record of it, leaving the
--- Session as what is stored on disk and resumable like any other history.
function Client:close(id)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  return operate(self, "close_interaction", { id = named }, protocol.Response.ACCEPTED)
end

--- Switch a live interaction onto another model, now and for its next
--- resume. The provider cannot change; that needs a new session.
function Client:set_selection(id, selection)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  local picked, selection_error = M.selection(selection)
  if not picked then
    return nil, selection_error
  end
  return operate(
    self,
    "set_session_selection",
    { id = named, selection = picked },
    protocol.Response.ACCEPTED
  )
end

--- Change the directory later turns run in. The path is on the host and must
--- stay inside the interaction's Workspace.
function Client:set_working_directory(id, directory)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  local path, path_error = text_argument(directory, "the working directory")
  if not path then
    return nil, path_error
  end
  return operate(
    self,
    "set_interaction_working_directory",
    { id = named, directory = path },
    protocol.Response.ACCEPTED
  )
end

--- Keep at it after a rate limit, or stop doing so: ask the same turn again
--- once the plan window turns over.
function Client:set_auto_retry(id, enabled)
  local named, err = text_argument(id, "the interaction id")
  if not named then
    return nil, err
  end
  local flag, flag_error = flag_argument(enabled, "auto_retry")
  if flag == nil then
    return nil, flag_error
  end
  return operate(
    self,
    "set_interaction_auto_retry",
    { id = named, enabled = flag },
    protocol.Response.ACCEPTED
  )
end

-- Answers
-- -------

--- Parse the session's most recent agent message under a contract and return
--- the typed `Answer`, taking the contract of its last typed turn by default.
---
--- A reply that did not satisfy its contract is an `Answer` too, not an error
--- in place of one: `answer.value` is absent, `answer.error` says what was
--- wrong, and `answer.source` still carries what the agent actually said.
---@param id string
---@param options? { contract?: string }
function Client:answer(id, options)
  local named, err = text_argument(id, "the session id")
  if not named then
    return nil, err
  end
  options = options or {}
  local contract, contract_error
  if options.contract then
    contract, contract_error = M.contract(options.contract)
    if not contract then
      return nil, contract_error
    end
  end
  return operate(self, "turn_answer", { id = named, contract = contract }, protocol.Response.ANSWER)
end

--- The value an `Answer` carries, whatever its contract: the text, the lines,
--- the file locations, the decoded JSON. `nil` and the reason when the reply
--- missed its contract — `answer.source` is still worth showing then.
function M.answered(answer)
  local value = M.given(answer.value)
  if not value then
    return nil, M.given(answer.error) or "the reply did not satisfy its contract"
  end
  return value.value, nil
end

-- Following a turn
-- ----------------

--- Watch an interaction's updates, without blocking the editor.
---
--- A turn takes minutes, and a connection carries one request, so a client
--- watching one polls: this asks for updates every `interval` milliseconds
--- from a timer, hands each to `on_update(update, sequence)`, and stops when
--- the interaction ends or when the returned handle's `stop` is called.
---
---   local watching = styra:follow("styra-7", {}, function(update)
---     if update.type == "event" then ... end
---   end)
---   watching.stop()
---
--- `options.after` is the sequence to start after, 0 (the whole journal) by
--- default. `options.on_error(message)` is called if a poll fails, after which
--- following stops; `options.on_end(ending)` is called when the interaction
--- ends.
---@param id string
---@param options? { after?: integer, interval?: integer, raw?: boolean, on_error?: function, on_end?: function }
---@param on_update fun(update: table, sequence: integer)
---@return table handle
function Client:follow(id, options, on_update)
  options = options or {}
  if type(on_update) ~= "function" then
    error("svara: following an interaction needs a function to hand updates to", 2)
  end
  local after = options.after or 0
  local interval = options.interval or DEFAULT_INTERVAL
  local handle = { stopped = false }
  local cancel

  function handle.stop()
    handle.stopped = true
    if cancel then
      cancel()
      cancel = nil
    end
  end

  local function fail(message)
    handle.stop()
    if options.on_error then
      options.on_error(message)
    end
  end

  local poll
  function poll()
    if handle.stopped then
      return
    end
    local updates, err = self:updates(id, after, { raw = options.raw })
    if not updates then
      return fail(err)
    end
    after = updates.next
    handle.after = after
    for _, sequenced in ipairs(updates.updates) do
      if handle.stopped then
        return
      end
      on_update(sequenced.update, sequenced.sequence)
      if sequenced.update.type == protocol.InteractionUpdate.ENDED then
        handle.stop()
        if options.on_end then
          options.on_end(sequenced.update.data)
        end
        return
      end
    end
    if not handle.stopped then
      cancel = self.host.defer(interval, poll)
    end
  end

  poll()
  return handle
end

--- Ask one question and be handed the typed answer when the turn is done.
---
--- The three requests this takes are the protocol's own shape, not a quirk of
--- this module: `send_message` names the contract, `updates` is polled until
--- the turn completes, and `turn_answer` parses the reply the server already
--- has. Nothing blocks — `on_answer(value, answer)` is called from a timer
--- once there is something to say, or `on_answer(nil, nil, error)` if there
--- never will be.
---
---   styra:ask("styra-7", "which files handle auth?", { contract = "files" },
---     function(files, answer, err) ... end)
---
--- The baseline it polls from is read *before* the message goes out, so a
--- turn completed before this call cannot be mistaken for this one's.
---@param id string
---@param text string
---@param options? { contract?: string, selection?: string|table, interval?: integer }
---@param on_answer fun(value: any, answer: table?, error: string?)
---@return table? handle
---@return string? error
function Client:ask(id, text, options, on_answer)
  options = options or {}
  if type(on_answer) ~= "function" then
    error("svara: asking needs a function to hand the answer to", 2)
  end
  local contract = options.contract or protocol.Contract.TEXT

  local baseline, err = self:updates(id, 0, { raw = false })
  if not baseline then
    return nil, err
  end

  local sent, send_error = self:send_message(id, text, {
    contract = contract,
    selection = options.selection,
  })
  if not sent then
    return nil, send_error
  end

  local handle, finished
  local function answered(message)
    if finished then
      return
    end
    finished = true
    -- The turn may already be over, in which case this runs inside the first
    -- poll and there is no handle to stop yet; the caller-side stop below
    -- catches that case rather than this one guessing at it.
    if handle then
      handle.stop()
    end
    if message then
      return on_answer(nil, nil, message)
    end
    local answer, answer_error = self:answer(id, { contract = contract })
    if not answer then
      return on_answer(nil, nil, answer_error)
    end
    local value, missed = M.answered(answer)
    -- A reply that missed its contract is still an answer, and the caller is
    -- handed it either way: what was said is worth reading even when it could
    -- not be parsed.
    return on_answer(value, answer, missed)
  end

  handle = self:follow(id, {
    after = baseline.next,
    interval = options.interval,
    raw = false,
    on_error = answered,
    on_end = function()
      answered("the interaction ended before it answered")
    end,
  }, function(update)
    if update.type ~= protocol.InteractionUpdate.EVENT then
      return
    end
    local event = update.data
    if event.type == protocol.AgentEvent.TURN_COMPLETED then
      answered(nil)
    elseif event.type == protocol.AgentEvent.ERROR then
      answered(event.message)
    end
  end)
  if finished then
    handle.stop()
  end
  return handle
end

return M
