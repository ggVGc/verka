local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

local uv = vim.uv or vim.loop
local socket = vim.fn.tempname() .. ".sock"
local received
local server = assert(uv.new_pipe(false))
assert(server:bind(socket))
server:listen(4, function(listen_error)
  assert(not listen_error, listen_error)
  local peer = assert(uv.new_pipe(false))
  server:accept(peer)
  local request = ""
  peer:read_start(function(read_error, chunk)
    assert(not read_error, read_error)
    if not chunk then
      return
    end
    request = request .. chunk
    if request:find("\n", 1, true) then
      received = vim.json.decode(request)
      local response = '{"status":"ok","response":{"type":"accepted"}}\n'
      if received.data.id == "missing" then
        response = '{"status":"error","error":"unknown interaction"}\n'
      end
      peer:write(response, function(write_error)
        assert(not write_error, write_error)
        peer:read_stop()
        peer:close()
      end)
    end
  end)
end)

local sent, err = require("svara").send_message("styra-7", "review this buffer", {
  socket = socket,
  timeout = 1000,
})
assert(sent, err)
assert(received.operation == "send_message")
assert(received.data.id == "styra-7")
assert(received.data.message.text == "review this buffer")
assert(received.data.message.selection == nil)

local rejected, rejection = require("svara").send_message("missing", "hello", {
  socket = socket,
  timeout = 1000,
})
assert(not rejected)
assert(rejection == "unknown interaction")

-- What `:Svara` puts in front of a prompt.
local core = require("svara.core")
assert(core.viewing() == nil, "an unnamed buffer is no location")
assert(core.prompt_from_view("fix this", nil) == "fix this")

local file = vim.fn.tempname() .. ".lua"
vim.fn.writefile({ "local a = 1", "local b = 2", "local c = 3" }, file)
vim.cmd.edit(file)
vim.api.nvim_win_set_cursor(0, { 2, 0 })
assert(core.viewing() == file .. ":2", core.viewing())
assert(core.prompt_from_view("fix this", core.viewing()) == file .. ":2\n\nfix this")
os.remove(file)

server:close()
os.remove(socket)
print("svara core tests passed")
