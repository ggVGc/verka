local M = {}

local uv = vim.uv or vim.loop

local function socket_path(options)
  if options.socket and options.socket ~= "" then
    return options.socket
  end

  local configured = os.getenv("STYRA_SOCKET")
  if configured and configured ~= "" then
    return configured
  end

  local runtime = os.getenv("XDG_RUNTIME_DIR")
  if not runtime or runtime == "" then
    return nil, "XDG_RUNTIME_DIR is not set; pass options.socket or set STYRA_SOCKET"
  end
  return runtime .. "/styra/styra.sock"
end

local function exchange(path, request, timeout)
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

    pipe:write(request, function(write_error)
      if write_error then
        finish("writing Styra request: " .. write_error)
      end
    end)
  end)

  if not vim.wait(timeout, function()
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
  local line = wire:match("^([^\n]*)\n")
  if not line then
    return nil, "Styra closed the connection without a newline-terminated response"
  end
  return line
end

---Send a message to an existing, live Styra session.
---@param session_id string
---@param message string
---@param options? { socket?: string, timeout?: integer }
---@return boolean? sent
---@return string? error
function M.send_message(session_id, message, options)
  if type(session_id) ~= "string" or session_id == "" then
    return nil, "session_id must be a non-empty string"
  end
  if type(message) ~= "string" or message == "" then
    return nil, "message must be a non-empty string"
  end

  options = options or {}
  local path, path_error = socket_path(options)
  if not path then
    return nil, path_error
  end

  local request = vim.json.encode({
    operation = "send_message",
    data = {
      id = session_id,
      message = { text = message },
    },
  }) .. "\n"

  local line, transport_error = exchange(path, request, options.timeout or 5000)
  if not line then
    return nil, transport_error
  end

  local decoded_ok, response = pcall(vim.json.decode, line)
  if not decoded_ok or type(response) ~= "table" then
    return nil, "Styra returned invalid JSON"
  end
  if response.status == "error" then
    return nil, response.error or "Styra rejected the request"
  end
  if response.status ~= "ok"
    or type(response.response) ~= "table"
    or response.response.type ~= "accepted"
  then
    return nil, "Styra returned an unexpected response"
  end
  return true
end

return M
