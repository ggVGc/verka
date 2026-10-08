-- The Neovim host, against a real socket and a real timer.
--
-- `tests/api_spec.lua` runs the API on a host of its own, which is the point
-- of there being a host at all. This is the other half: the four things
-- `svara.nvim` actually does with Neovim, checked where they cannot be faked —
-- a Unix socket carrying one request and one reply, and a timer that fires on
-- the event loop.
--
--   nvim --headless -u NONE -l tests/nvim_spec.lua

local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

local api = require("svara.api")
local host = require("svara.nvim")
local uv = vim.uv or vim.loop

-- A server ---------------------------------------------------------------

--- A Styra-shaped server: one connection, one line in, one line out, with
--- each reply taken in turn from `replies`.
local function serve(replies)
  local path = vim.fn.tempname() .. ".sock"
  local server = assert(uv.new_pipe(false))
  assert(server:bind(path))
  local received = {}
  server:listen(8, function(listen_error)
    assert(not listen_error, listen_error)
    local peer = assert(uv.new_pipe(false))
    server:accept(peer)
    local line = ""
    peer:read_start(function(read_error, chunk)
      assert(not read_error, read_error)
      if not chunk then
        return
      end
      line = line .. chunk
      if line:find("\n", 1, true) then
        received[#received + 1] = vim.json.decode(line)
        local reply = table.remove(replies, 1) or { status = "error", error = "out of replies" }
        peer:write(vim.json.encode(reply) .. "\n", function(write_error)
          assert(not write_error, write_error)
          peer:read_stop()
          peer:close()
        end)
      end
    end)
  end)
  return path, server, received
end

-- Timers -----------------------------------------------------------------

do
  local fired = false
  host.defer(10, function()
    fired = true
  end)
  assert(vim.wait(1000, function()
    return fired
  end, 5), "the deferred function never ran")

  local cancelled = false
  local cancel = host.defer(10, function()
    cancelled = true
  end)
  cancel()
  vim.wait(50)
  assert(not cancelled, "a cancelled timer ran anyway")
end

-- One exchange -----------------------------------------------------------

do
  local path, server = serve({
    { status = "ok", response = { type = "health", data = { service = "styra-server" } } },
  })
  local styra = assert(api.open({ socket = path, timeout = 2000 }))
  local health, err = styra:health()
  assert(health, err)
  assert(health.service == "styra-server")
  server:close()
  os.remove(path)
end

do
  -- A socket nothing is listening on is a message, not a hang.
  local styra = assert(api.open({ socket = vim.fn.tempname() .. ".sock", timeout = 500 }))
  local health, err = styra:health()
  assert(not health)
  assert(err:find("connecting to Styra socket"), err)
end

-- An exchange inside `run` waits without holding the editor --------------

do
  local path, server = serve({
    { status = "ok", response = { type = "health", data = { service = "styra-server" } } },
    { status = "ok", response = { type = "health", data = { service = "styra-server" } } },
  })
  local styra = assert(api.open({ socket = path, timeout = 2000 }))
  local order = {}
  host.run(function()
    order[#order + 1] = "asked"
    local health, err = styra:health()
    order[#order + 1] = health and health.service or err
  end)
  -- `run` came back at the exchange, before the reply could arrive: the
  -- editor is free while the server works.
  order[#order + 1] = "editor free"
  assert(vim.wait(2000, function()
    return #order == 3
  end, 5), vim.inspect(order))
  assert(vim.deep_equal(order, { "asked", "editor free", "styra-server" }), vim.inspect(order))

  -- Outside `run`, and inside any coroutine `run` did not start, it blocks
  -- as before: a coroutine some other plugin is running is not yielded out of.
  local inside = coroutine.wrap(function()
    return styra:health()
  end)
  local health = inside()
  assert(health and health.service == "styra-server", vim.inspect(health))

  server:close()
  os.remove(path)
end

do
  -- Nothing listening is a message inside `run` too, not a coroutine left
  -- waiting forever.
  local styra = assert(api.open({ socket = vim.fn.tempname() .. ".sock", timeout = 500 }))
  local err
  host.run(function()
    local _
    _, err = styra:health()
  end)
  assert(vim.wait(2000, function()
    return err ~= nil
  end, 5), "the failed exchange never came back")
  assert(err:find("connecting to Styra socket"), err)
end

do
  -- Nor does a server that never answers leave it waiting: the timeout holds.
  local path = vim.fn.tempname() .. ".sock"
  local silent = assert(uv.new_pipe(false))
  assert(silent:bind(path))
  local held = {}
  silent:listen(8, function()
    local peer = assert(uv.new_pipe(false))
    silent:accept(peer)
    held[#held + 1] = peer
  end)
  local styra = assert(api.open({ socket = path, timeout = 100 }))
  local err
  host.run(function()
    local _
    _, err = styra:health()
  end)
  assert(vim.wait(2000, function()
    return err ~= nil
  end, 5), "the timeout never fired")
  assert(err:find("timed out"), err)
  for _, peer in ipairs(held) do
    peer:close()
  end
  silent:close()
  os.remove(path)
end

-- A turn, followed on the loop -------------------------------------------

do
  local path, server, received = serve({
    { status = "ok", response = { type = "updates", data = { updates = {}, next = 4 } } },
    { status = "ok", response = { type = "accepted" } },
    { status = "ok", response = { type = "updates", data = { updates = {}, next = 4 } } },
    { status = "ok", response = { type = "updates", data = {
      updates = { {
        sequence = 5,
        update = { type = "event", data = { type = "turn_completed", usage = {} } },
      } },
      next = 5,
    } } },
    { status = "ok", response = { type = "answer", data = {
      contract = "text",
      value = { contract = "text", value = "src/auth.rs is the one" },
      source = "<styra:answer>src/auth.rs is the one</styra:answer>",
    } } },
  })

  local styra = assert(api.open({ socket = path, timeout = 2000 }))
  local answered
  local handle, err = styra:ask("styra-7", "which file handles auth?", { interval = 10 }, function(value, _, ask_error)
    answered = { value = value, error = ask_error }
  end)
  assert(handle, err)

  assert(vim.wait(2000, function()
    return answered ~= nil
  end, 5), "the turn was never answered")
  assert(answered.error == nil, tostring(answered.error))
  assert(answered.value == "src/auth.rs is the one", tostring(answered.value))

  -- Baseline, message, two polls, answer — and no poll after the answer.
  assert(#received == 5, vim.inspect(vim.tbl_map(function(request)
    return request.operation
  end, received)))
  vim.wait(100)
  assert(#received == 5, "the answered turn is still being polled")

  server:close()
  os.remove(path)
end

print("svara nvim host tests passed")
