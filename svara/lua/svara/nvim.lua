-- Neovim, as the four things `svara.api` needs from a host.
--
-- The API is the protocol plus the two pieces the generated library leaves to
-- its callers — a JSON codec and a Unix socket — and a client living inside an
-- editor also needs a clock to poll with. All of that is editor-specific, and
-- all of it is here rather than spread through `svara.api`, so the API itself
-- mentions Neovim nowhere and a test can hand it another host:
--
--   local styra = require("svara.api").open({ host = fake })
--
-- A host is a table with these fields. Anything implementing them will do.
--
--   encode(value)                -> string            JSON, one line's worth
--   decode(text)                 -> value             raising on bad JSON
--   null                          = the decoder's null, the value a field the
--                                   server sent as null decodes to, and the
--                                   value to send where null means something
--   getenv(name)                 -> string|nil
--   exchange(path, line, timeout) -> line|nil, error   one whole connection:
--                                   connect, write one line, read one, close
--   defer(milliseconds, fn)      -> cancel()          run fn later, once
--
-- `exchange` blocks, which is what one short request/response round trip
-- wants; `defer` is what keeps a minutes-long turn from blocking with it.
--
-- Except inside `M.run`: there an exchange waits by yielding instead, and the
-- editor carries on until the reply comes. That is for the request that is
-- not short — a launch that makes a Git worktree and names its branch can take
-- many seconds — and is asked for rather than assumed, since a coroutine some
-- other plugin is running would not expect to be yielded out of.

local uv = vim.uv or vim.loop

local M = {}

M.null = vim.NIL

-- The coroutines `M.run` started, which are the only ones an exchange will
-- yield out of.
local running = setmetatable({}, { __mode = "k" })

function M.encode(value)
  return vim.json.encode(value)
end

function M.decode(text)
  return vim.json.decode(text)
end

function M.getenv(name)
  return os.getenv(name)
end

--- Write one line to the Unix socket and read one back.
---
--- A connection carries exactly one request and one response, so an exchange
--- is a whole connection: connect, write one line, read one line, done.
function M.exchange(path, line, timeout)
  local co = coroutine.running()
  local yielding = co ~= nil and running[co] == true
  local waiting = false

  local pipe = uv.new_pipe(false)
  if not pipe then
    return nil, "could not create a Unix socket client"
  end

  local chunks = {}
  local done = false
  local transport_error

  local function finish(err)
    if done then
      return
    end
    done = true
    transport_error = err
    pcall(pipe.read_stop, pipe)
    if waiting then
      vim.schedule(function()
        local resumed, failure = coroutine.resume(co)
        if not resumed then
          error(failure)
        end
      end)
    end
  end

  pipe:connect(path, function(connect_error)
    if done then
      return
    end
    if connect_error then
      finish("connecting to Styra socket " .. path .. ": " .. connect_error)
      return
    end

    pipe:read_start(function(read_error, chunk)
      if read_error then
        finish("reading Styra response: " .. read_error)
      elseif chunk then
        chunks[#chunks + 1] = chunk
        if table.concat(chunks):find("\n", 1, true) then
          finish()
        end
      else
        finish()
      end
    end)

    pipe:write(line .. "\n", function(write_error)
      if write_error then
        finish("writing Styra request: " .. write_error)
      end
    end)
  end)

  if yielding then
    local timer = assert(uv.new_timer())
    timer:start(timeout, 0, function()
      finish("timed out waiting for Styra after " .. timeout .. "ms")
    end)
    if not done then
      waiting = true
      coroutine.yield()
    end
    timer:stop()
    timer:close()
  elseif not vim.wait(timeout, function()
    return done
  end, 10) then
    finish("timed out waiting for Styra after " .. timeout .. "ms")
  end

  if not pipe:is_closing() then
    pipe:close()
  end
  if transport_error then
    return nil, transport_error
  end

  local wire = table.concat(chunks)
  local reply = wire:match("^([^\n]*)\n")
  if not reply then
    return nil, "Styra closed the connection without a newline-terminated response"
  end
  return reply
end

--- Run `fn` now, letting every exchange it makes wait without blocking the
--- editor: `fn` returns at its first exchange and carries on when the reply
--- comes. Whatever runs `fn` gets no result back; `fn` does what it does with
--- its own.
---@param fn fun()
function M.run(fn)
  local co = coroutine.create(fn)
  running[co] = true
  local resumed, failure = coroutine.resume(co)
  if not resumed then
    error(failure)
  end
end

--- Run `fn` once, `milliseconds` from now, and return a function cancelling it.
function M.defer(milliseconds, fn)
  local timer = uv.new_timer()
  if not timer then
    error("svara: could not create a timer")
  end
  timer:start(milliseconds, 0, function()
    timer:stop()
    if not timer:is_closing() then
      timer:close()
    end
    vim.schedule(fn)
  end)
  return function()
    if not timer:is_closing() then
      timer:stop()
      timer:close()
    end
  end
end

return M
