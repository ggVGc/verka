#!/usr/bin/env lua
--
-- A small Styra client, written against the generated protocol library.
--
--   lua examples/styra-ask.lua health
--   lua examples/styra-ask.lua workspaces
--   lua examples/styra-ask.lua interactions
--   lua examples/styra-ask.lua ask styra-1 which files handle auth?
--
-- It exists to show what the library does and does not do. Every operation
-- name, field name, and enum spelling below comes from `styra.protocol`, which
-- is generated from the Rust that defines the protocol — nothing here restates
-- the wire format, and a request this file gets wrong is refused on the line
-- that built it rather than by the server a round trip later.
--
-- What is left is exactly what the Rust crate leaves to its callers too: a JSON
-- codec and a socket. Both are picked up from whatever the machine has, in the
-- handful of lines under "Transport" — that part is this example's, not the
-- protocol's.
--
-- Needs LuaSocket with Unix-socket support or `socat` on PATH, and uses
-- lua-cjson or dkjson if either is installed, falling back to the toy codec in
-- `json.lua` beside it. Point it at a server with --socket PATH; the default is
-- $XDG_RUNTIME_DIR/styra/styra.sock, the same one `styractl` uses.

package.path = table.concat({
  -- The library and this file's neighbours, found relative to the script so it
  -- runs from anywhere without being installed.
  ((arg[0]:match("^(.*)/[^/]+$") or ".") .. "/../?.lua"),
  package.path,
}, ";")

local protocol = require("styra.protocol")

-- JSON ------------------------------------------------------------------

-- Whatever the machine has, falling back to the toy codec beside this file so
-- the example runs anywhere.
local json
for _, name in ipairs({ "cjson", "dkjson", "examples.json" }) do
  local found, module = pcall(require, name)
  if found then
    json = module
    break
  end
end

-- The one thing the protocol needs the codec's help with: Lua cannot tell a
-- key that is absent from one set to nil, so nullable fields are said with
-- `protocol.null`. Teaching it the encoder's own null is all it takes.
protocol.use_null(json.null or protocol.null)

--- And the same in reverse on the way back: a field the server sent as null
--- decodes to that sentinel, which is a value in Lua rather than an absence,
--- so anything read for display goes through here first.
local function given(value)
  if value == nil or value == protocol.null then
    return nil
  end
  return value
end

-- Transport -------------------------------------------------------------

-- A connection carries exactly one request and one response, so an exchange is
-- a whole connection: connect, write one line, read one line, done.

local function socket_path(explicit)
  if explicit then
    return explicit
  end
  local runtime = os.getenv("XDG_RUNTIME_DIR")
  if not runtime or runtime == "" then
    io.stderr:write("XDG_RUNTIME_DIR is not set; pass --socket explicitly\n")
    os.exit(1)
  end
  return runtime .. "/styra/styra.sock"
end

local function luasocket_exchange(path, line)
  local found, unix = pcall(require, "socket.unix")
  if not found then
    return nil
  end
  local client = type(unix) == "function" and unix() or unix.stream()
  local connected, err = client:connect(path)
  if not connected then
    return nil, string.format("connecting to %s: %s", path, tostring(err))
  end
  client:send(line .. "\n")
  local reply, receive_error = client:receive("*l")
  client:close()
  if not reply then
    return nil, string.format("reading from %s: %s", path, tostring(receive_error))
  end
  return reply
end

local function have(program)
  local pipe = io.popen("command -v " .. program .. " 2>/dev/null")
  if not pipe then
    return false
  end
  local found = pipe:read("*l")
  pipe:close()
  return found ~= nil and found ~= ""
end

local function socat_exchange(path, line)
  if not have("socat") then
    return nil
  end
  local request_file, reply_file = os.tmpname(), os.tmpname()
  local handle = assert(io.open(request_file, "w"))
  handle:write(line, "\n")
  handle:close()
  os.execute(string.format(
    "socat -t 30 STDIO UNIX-CONNECT:%q < %q > %q 2>/dev/null",
    path,
    request_file,
    reply_file
  ))
  local reply = io.open(reply_file, "r")
  local first = reply and reply:read("*l") or nil
  if reply then
    reply:close()
  end
  os.remove(request_file)
  os.remove(reply_file)
  if not first then
    return nil, string.format("socat got no reply from %s", path)
  end
  return first
end

--- Send one request table and return the decoded reply, or nil and a message.
local function exchange(path, request)
  local line = json.encode(request)
  local reply, err = luasocket_exchange(path, line)
  if not reply and not err then
    reply, err = socat_exchange(path, line)
  end
  if not reply then
    return nil, err or "no way to reach a Unix socket; install LuaSocket or socat"
  end
  return json.decode(reply)
end

--- Send one request and return the data of the response it must answer with.
local function call(path, request, expected)
  local reply, err = exchange(path, request)
  if not reply then
    return nil, err
  end
  return protocol.expect(reply, expected)
end

-- Commands --------------------------------------------------------------

local commands = {}

function commands.health(path)
  local health, err = call(path, protocol.request.health(), protocol.Response.HEALTH)
  if not health then
    return nil, err
  end
  print(string.format("%s is up", health.service))
  return true
end

function commands.workspaces(path)
  local workspaces, err =
    call(path, protocol.request.list_workspaces(), protocol.Response.WORKSPACES)
  if not workspaces then
    return nil, err
  end
  for _, workspace in ipairs(workspaces) do
    print(string.format(
      "%s  %-30s %s  (%d sessions)",
      workspace.id,
      given(workspace.name) or "-",
      workspace.host_path,
      workspace.session_count
    ))
  end
  return true
end

function commands.interactions(path)
  local interactions, err =
    call(path, protocol.request.list_interactions(), protocol.Response.INTERACTIONS)
  if not interactions then
    return nil, err
  end
  for _, interaction in ipairs(interactions) do
    print(string.format(
      "%s  %-10s %s:%s/%s  %s",
      interaction.id,
      given(interaction.activity) or protocol.InteractionActivity.PENDING,
      interaction.selection.provider,
      interaction.selection.model,
      interaction.selection.effort,
      given(interaction.last_message) or ""
    ))
  end
  return true
end

--- Ask one question under a contract, wait for the turn, print the answer.
---
--- The three requests this takes are the protocol's own shape, not a quirk of
--- Lua: a connection carries one request, and a turn takes minutes, so the
--- answer is fetched rather than returned. `send_message` names the contract,
--- `updates` is polled until the turn ends, and `turn_answer` parses the
--- reply the server already has.
function commands.ask(path, session, ...)
  local question = table.concat({ ... }, " ")
  if not session or question == "" then
    return nil, "usage: ask <session> <question...>"
  end

  local sent, err = call(
    path,
    protocol.request.send_message({
      id = session,
      message = { text = question, contract = protocol.Contract.LINES },
    }),
    protocol.Response.ACCEPTED
  )
  if not sent then
    return nil, err
  end

  local after, ended = 0, false
  while not ended do
    local updates, updates_error = call(
      path,
      -- The raw wire lines dominate a long interaction's volume and this
      -- client renders none of them.
      protocol.request.updates({ id = session, after = after, raw = false }),
      protocol.Response.UPDATES
    )
    if not updates then
      return nil, updates_error
    end
    after = updates.next
    for _, sequenced in ipairs(updates.updates) do
      local update = sequenced.update
      if update.type == protocol.InteractionUpdate.EVENT then
        local event = update.data
        if event.type == protocol.AgentEvent.TURN_COMPLETED then
          ended = true
        elseif event.type == protocol.AgentEvent.ERROR then
          return nil, event.message
        end
      elseif update.type == protocol.InteractionUpdate.ENDED then
        return nil, "the interaction ended before it answered"
      end
    end
    if not ended then
      os.execute("sleep 0.25")
    end
  end

  local answer, answer_error =
    call(path, protocol.request.turn_answer({ id = session }), protocol.Response.ANSWER)
  if not answer then
    return nil, answer_error
  end
  -- A reply that missed its contract is still an answer: show what was said
  -- and why it could not be read, rather than nothing at all.
  if not given(answer.value) then
    io.stderr:write(string.format("the reply did not satisfy its contract: %s\n", answer.error))
    print(answer.source)
    return true
  end
  for _, item in ipairs(answer.value.value) do
    print(item)
  end
  return true
end

-- Entry point -----------------------------------------------------------

local function usage()
  io.stderr:write([[
usage: styra-ask.lua [--socket PATH] <command> [arguments]

  health                        check that the server is reachable
  workspaces                    list Workspaces
  interactions                  list live interactions
  ask <session> <question...>   ask one question and print the typed answer
]])
  os.exit(2)
end

local arguments, path = {}, nil
local index = 1
while arg[index] do
  if arg[index] == "--socket" then
    index = index + 1
    path = arg[index] or usage()
  else
    arguments[#arguments + 1] = arg[index]
  end
  index = index + 1
end

local command = commands[arguments[1] or ""]
if not command then
  usage()
end

local unpack_arguments = table.unpack or unpack
local ok, message = command(socket_path(path), unpack_arguments(arguments, 2))
if not ok then
  io.stderr:write(tostring(message), "\n")
  os.exit(1)
end
