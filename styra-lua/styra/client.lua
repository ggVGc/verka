-- The two things `styra.protocol` deliberately leaves to its callers.
--
-- The generated module is the wire vocabulary and nothing else: it builds and
-- checks request tables, and has no opinion about how those tables become
-- bytes or how the bytes reach the server. That is the right shape for a
-- generated library — but every Lua script talking to Styra then needs the
-- same two pieces, a JSON codec and a Unix socket, and writing them again per
-- script is how a set of small tools drifts apart.
--
-- So they live here instead, hand-written and not generated, sitting on top of
-- `styra.protocol` exactly as a script would:
--
--   local styra = require("styra.client").open()
--   local health = assert(styra:call(styra.protocol.request.health(),
--                                    styra.protocol.Response.HEALTH))
--
-- Neither piece is the protocol's, and both are replaceable: point `open` at
-- your own codec or exchange function and the rest keeps working.
--
-- Needs LuaSocket with Unix-socket support or `socat` on PATH, and uses
-- lua-cjson or dkjson if either is installed, falling back to the toy codec in
-- `styra/json.lua`.

local protocol = require("styra.protocol")

local M = {}

M.protocol = protocol

-- JSON -------------------------------------------------------------------

--- Whatever the machine has, falling back to the toy codec shipped beside this
--- file so a script runs anywhere.
function M.codec()
  for _, name in ipairs({ "cjson", "dkjson", "styra.json" }) do
    local found, module = pcall(require, name)
    if found then
      return module
    end
  end
  error("no JSON codec found, not even styra.json")
end

--- The value a field the server sent as null decodes to is a value in Lua, not
--- an absence, so anything read for display goes through here first.
function M.given(value)
  if value == nil or value == protocol.null then
    return nil
  end
  return value
end

-- Sockets ----------------------------------------------------------------

--- The socket `styractl` uses, unless told otherwise.
function M.default_socket()
  local runtime = os.getenv("XDG_RUNTIME_DIR")
  if not runtime or runtime == "" then
    return nil, "XDG_RUNTIME_DIR is not set; pass a socket path explicitly"
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

--- Write one line to the socket and read one back, however this machine can.
---
--- A connection carries exactly one request and one response, so an exchange
--- is a whole connection: connect, write one line, read one line, done.
function M.exchange(path, line)
  local reply, err = luasocket_exchange(path, line)
  if not reply and not err then
    reply, err = socat_exchange(path, line)
  end
  if not reply then
    return nil, err or "no way to reach a Unix socket; install LuaSocket or socat"
  end
  return reply
end

-- Clients ----------------------------------------------------------------

local Client = {}
Client.__index = Client

--- Open a client.
---
--- `options.socket` is the server's socket, defaulting to `styractl`'s.
--- `options.json` and `options.exchange` replace the codec and the transport,
--- which is the point of them being options at all: a script inside an editor
--- or a test harness has its own way to move bytes.
function M.open(options)
  options = options or {}
  local path = options.socket
  if not path then
    local default, err = M.default_socket()
    if not default then
      return nil, err
    end
    path = default
  end
  local json = options.json or M.codec()
  -- The one thing the protocol needs the codec's help with: Lua cannot tell a
  -- key that is absent from one set to nil, so nullable fields are said with
  -- `protocol.null`. Teaching it the encoder's own null is all it takes.
  protocol.use_null(json.null or protocol.null)
  return setmetatable({
    socket = path,
    json = json,
    protocol = protocol,
    given = M.given,
    _exchange = options.exchange or M.exchange,
  }, Client)
end

--- Send one request table and return the decoded reply, or nil and a message.
function Client:send(request)
  local reply, err = self._exchange(self.socket, self.json.encode(request))
  if not reply then
    return nil, err
  end
  return self.json.decode(reply)
end

--- Send one request and return the data of the response it must answer with,
--- or nil and the server's error. The expected response is named by the
--- caller so a surprise comes back as a message rather than a nil field three
--- lines later.
function Client:call(request, expected)
  local reply, err = self:send(request)
  if not reply then
    return nil, err
  end
  return protocol.expect(reply, expected)
end

--- Send one request and return the response table whatever it is, for the
--- callers that genuinely handle more than one outcome.
function Client:unwrap(request)
  local reply, err = self:send(request)
  if not reply then
    return nil, err
  end
  return protocol.unwrap(reply)
end

return M
