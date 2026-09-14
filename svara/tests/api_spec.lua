-- The API without a server, an editor, or a socket.
--
-- Everything Neovim-specific is a host (see `svara.nvim`), so a test can hand
-- `api.open` one of its own: a codec, a fake exchange answering from a script
-- of replies, and a timer that fires when this file says so. What is left
-- under test is the API itself — the requests it builds, the responses it
-- reads, and the polling it does over a turn.
--
--   nvim --headless -u NONE -l tests/api_spec.lua

local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

local api = require("svara.api")
local protocol = require("svara.protocol")

-- A host -----------------------------------------------------------------

--- A host answering each exchange from `replies`, remembering every request
--- it was given and every deferred callback it has not been told to run.
local function fake_host(replies)
  local host = { sent = {}, timers = {}, null = vim.NIL, environment = {} }

  function host.encode(value)
    return vim.json.encode(value)
  end

  function host.decode(text)
    return vim.json.decode(text)
  end

  function host.getenv(name)
    return host.environment[name]
  end

  function host.exchange(path, line, timeout)
    host.socket = path
    host.timeout = timeout
    local request = vim.json.decode(line)
    host.sent[#host.sent + 1] = request
    local reply = table.remove(replies, 1)
    if not reply then
      error("the test ran out of replies at " .. tostring(request.operation))
    end
    if type(reply) == "function" then
      reply = reply(request)
    end
    if type(reply) == "string" then
      return nil, reply
    end
    return vim.json.encode(reply)
  end

  function host.defer(milliseconds, fn)
    local timer = { at = milliseconds, run = fn, cancelled = false }
    host.timers[#host.timers + 1] = timer
    return function()
      timer.cancelled = true
    end
  end

  --- Run the pending timers, as the event loop would, and say how many fired.
  function host.tick()
    local pending = host.timers
    local fired = 0
    host.timers = {}
    for _, timer in ipairs(pending) do
      if not timer.cancelled then
        fired = fired + 1
        timer.run()
      end
    end
    return fired
  end

  function host.last()
    return host.sent[#host.sent]
  end

  return host
end

local function ok(data)
  return { status = "ok", response = data }
end

local function accepted()
  return ok({ type = "accepted" })
end

local function open(replies, options)
  options = vim.tbl_extend("force", { socket = "/tmp/test.sock" }, options or {})
  local host = fake_host(replies)
  options.host = host
  local styra, err = api.open(options)
  assert(styra, err)
  return styra, host
end

-- The socket -------------------------------------------------------------

do
  local host = fake_host({})
  host.environment.STYRA_SOCKET = "/run/elsewhere.sock"
  local styra = assert(api.open({ host = host }))
  assert(styra.socket == "/run/elsewhere.sock", styra.socket)

  host.environment.STYRA_SOCKET = nil
  host.environment.XDG_RUNTIME_DIR = "/run/user/1000"
  styra = assert(api.open({ host = host }))
  assert(styra.socket == "/run/user/1000/styra/styra.sock", styra.socket)

  host.environment.XDG_RUNTIME_DIR = nil
  local missing, err = api.open({ host = host })
  assert(not missing)
  assert(err:find("XDG_RUNTIME_DIR"), err)
end

-- Selections -------------------------------------------------------------

do
  local selection = assert(api.selection("claude:claude-opus-5/xhigh"))
  assert(selection.provider == "claude")
  assert(selection.model == "claude-opus-5")
  assert(selection.effort == "xhigh")
  assert(api.selection_name(selection) == "claude:claude-opus-5/xhigh")

  assert(api.selection({ provider = "codex", model = "gpt-5.6-terra", effort = "medium" }))

  local partial, err = api.selection("claude")
  assert(not partial)
  assert(err:find("reasoning effort"), err)

  partial, err = api.selection("claude/high")
  assert(not partial)
  assert(err:find("model"), err)

  local unknown
  unknown, err = api.selection("gemini:something/high")
  assert(not unknown)
  assert(err:find("not a provider"), err)

  unknown, err = api.selection("claude:claude-opus-5/gentle")
  assert(not unknown)
  assert(err:find("not a reasoning effort"), err)

  unknown, err = api.selection({ provider = "claude", model = "claude-opus-5" })
  assert(not unknown)
  assert(err:find("effort"), err)
end

-- Requests ---------------------------------------------------------------

do
  local styra, host = open({
    ok({ type = "health", data = { service = "styra-server" } }),
    ok({ type = "workspaces", data = {} }),
    accepted(),
  })

  local health = assert(styra:health())
  assert(health.service == "styra-server")
  assert(host.socket == "/tmp/test.sock")
  assert(host.last().operation == "health")
  assert(host.last().data == nil)

  assert(styra:workspaces())
  assert(host.last().operation == "list_workspaces")

  assert(styra:send_message("styra-7", "review this buffer", { contract = "lines" }))
  local sent = host.last()
  assert(sent.operation == "send_message")
  assert(sent.data.id == "styra-7")
  assert(sent.data.message.text == "review this buffer")
  assert(sent.data.message.contract == "lines")
end

-- Nulls are said, not left out ------------------------------------------

do
  local styra, host = open({
    ok({ type = "session_renamed", data = {} }),
    ok({ type = "workspace_git_repository_updated", data = {} }),
  })

  assert(styra:rename_session("styra-7", nil))
  assert(host.last().data.name == vim.NIL, vim.inspect(host.last()))

  assert(styra:set_workspace_git_repository("workspace-1", nil))
  assert(host.last().data.git_repository == vim.NIL)
end

-- A bad request never reaches the socket --------------------------------

do
  local styra, host = open({})

  local sent, err = styra:send_message("", "hello")
  assert(not sent)
  assert(err:find("non%-empty"), err)

  sent, err = styra:send_message("styra-7", "hello", { contract = "prose" })
  assert(not sent)
  assert(err:find("not a contract"), err)

  sent, err = styra:set_auto_retry("styra-7", "yes")
  assert(not sent)
  assert(err:find("true or false"), err)

  sent, err = styra:create_session("workspace-1", "claude:claude-opus-5/xhigh", {
    launch = { network = "on" },
  })
  assert(not sent)
  assert(err:find("network"), err)

  assert(#host.sent == 0, "a request the server would refuse was sent anyway")
end

-- The server's own errors ------------------------------------------------

do
  local styra = open({
    { status = "error", error = "unknown interaction" },
    ok({ type = "accepted" }),
    "connecting to Styra socket: no such file or directory",
  })

  local sent, err = styra:send_message("missing", "hello")
  assert(not sent)
  assert(err == "unknown interaction", err)

  -- A response about something else is a message, not a nil field later on.
  local health
  health, err = styra:health()
  assert(not health)
  assert(err:find("expected a \"health\" response"), err)

  local workspaces
  workspaces, err = styra:workspaces()
  assert(not workspaces)
  assert(err:find("no such file"), err)
end

-- Queues -----------------------------------------------------------------

do
  local styra, host = open({
    ok({ type = "queued_messages", data = { { text = "later" } } }),
    ok({ type = "sent_queued_message", data = { { text = "later" }, {} } }),
    ok({ type = "sent_queued_message", data = { vim.NIL, {} } }),
    ok({ type = "queued", data = 2 }),
  })

  local queued = assert(styra:queue_message("styra-7", "later", { contract = "text" }))
  assert(host.last().operation == "queue_message")
  assert(queued[1].text == "later")

  local drained = assert(styra:send_queued_message("styra-7"))
  assert(drained.sent.text == "later")
  assert(#drained.queued == 0)

  drained = assert(styra:send_queued_message("styra-7"))
  assert(drained.sent == nil)

  assert(styra:clear_queued_messages("styra-7") == 2)
end

-- Following a turn -------------------------------------------------------

local function sequenced(from, updates)
  local list = {}
  for index, update in ipairs(updates) do
    list[index] = { sequence = from + index, update = update }
  end
  return { updates = list, next = from + #updates }
end

do
  local styra, host = open({
    ok({ type = "updates", data = sequenced(0, {
      { type = "event", data = { type = "agent_message", text = "working" } },
    }) }),
    ok({ type = "updates", data = sequenced(1, {
      { type = "ended", data = { exit_code = 0, error = vim.NIL } },
    }) }),
    ok({ type = "updates", data = { updates = {}, next = 2 } }),
  })

  local seen, ending = {}, nil
  styra:follow("styra-7", {
    interval = 10,
    on_end = function(reason)
      ending = reason
    end,
  }, function(update)
    seen[#seen + 1] = update.type
  end)
  assert(#seen == 1, vim.inspect(seen))
  assert(host.last().data.after == 0)

  assert(host.tick() == 1)
  assert(#seen == 2)
  assert(seen[2] == "ended")
  assert(host.sent[#host.sent].data.after == 1)

  -- An ended interaction stops the polling rather than asking forever, and
  -- says so once.
  assert(host.tick() == 0)
  assert(ending and ending.exit_code == 0, vim.inspect(ending))

  -- So does a watch the caller stopped itself.
  local stopped = styra:follow("styra-7", { interval = 10 }, function() end)
  stopped.stop()
  assert(host.tick() == 0)
end

do
  -- A poll that fails stops the watch and says so once.
  local styra = open({ "the socket went away" })
  local failures = {}
  styra:follow("styra-7", {
    on_error = function(message)
      failures[#failures + 1] = message
    end,
  }, function() end)
  assert(#failures == 1)
  assert(failures[1] == "the socket went away")
end

-- Asking -----------------------------------------------------------------

do
  local styra, host = open({
    ok({ type = "updates", data = { updates = {}, next = 12 } }),
    accepted(),
    ok({ type = "updates", data = sequenced(12, {
      { type = "event", data = { type = "turn_started" } },
    }) }),
    ok({ type = "updates", data = sequenced(13, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "answer", data = {
      contract = "lines",
      value = { contract = "lines", value = { "src/auth.rs", "src/token.rs" } },
      source = "<styra:answer>…</styra:answer>",
    } }),
  })

  local answered
  local handle = assert(styra:ask("styra-7", "which files handle auth?", {
    contract = "lines",
    interval = 10,
  }, function(value, answer, err)
    answered = { value = value, answer = answer, error = err }
  end))

  -- The baseline is read before the message goes out, so a turn that finished
  -- before the question cannot be mistaken for its answer.
  assert(host.sent[1].operation == "updates")
  assert(host.sent[2].operation == "send_message")
  assert(host.sent[2].data.message.contract == "lines")
  assert(host.sent[3].data.after == 12)
  assert(answered == nil)

  host.tick()
  assert(answered, "the turn completed and nothing was said")
  assert(answered.error == nil, answered.error)
  assert(answered.value[1] == "src/auth.rs")
  assert(answered.answer.contract == "lines")
  assert(handle.stopped)
  assert(host.tick() == 0)
end

do
  -- A reply that missed its contract is still an answer.
  local styra, host = open({
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
    accepted(),
    ok({ type = "updates", data = sequenced(0, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "answer", data = {
      contract = "json",
      value = vim.NIL,
      error = "no answer block",
      source = "I could not do that",
    } }),
  })

  local answered
  styra:ask("styra-7", "a question", { contract = "json" }, function(value, answer, err)
    answered = { value = value, answer = answer, error = err }
  end)
  assert(answered, "the turn was already over and nothing was said")
  assert(answered.value == nil)
  assert(answered.error == "no answer block", tostring(answered.error))
  assert(answered.answer.source == "I could not do that")
  assert(host.tick() == 0, "the answered turn is still being polled")
end

do
  -- An interaction that ends mid-turn answers with why, not with silence.
  local styra = open({
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
    accepted(),
    ok({ type = "updates", data = sequenced(0, {
      { type = "ended", data = { exit_code = 1, error = vim.NIL } },
    }) }),
  })

  local answered
  styra:ask("styra-7", "a question", {}, function(value, answer, err)
    answered = err
  end)
  assert(answered and answered:find("ended before"), tostring(answered))
end

do
  -- Nothing is polled when the question itself was refused.
  local styra, host = open({
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
    { status = "error", error = "not accepting messages" },
  })

  local handle, err = styra:ask("styra-7", "a question", {}, function() end)
  assert(not handle)
  assert(err == "not accepting messages", err)
  assert(#host.timers == 0)
end

print("svara api tests passed")
