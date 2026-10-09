-- The requests still waiting on Styra, in a corner of the editor.
--
-- A question asked from `:SvaraAsk` is a whole turn of the agent's, and can
-- take minutes. Nothing in the editor is held while it runs, so something has
-- to say that it is running — otherwise the quickfix list changing under the
-- operator a few minutes later is the first they hear of it again.
--
-- So every request under way is one line in a small float at the top right:
-- a spinner, what was asked, and how long it has been. The float never takes
-- the focus and is not focusable, so it can sit there while the operator
-- carries on, and it closes itself once the last request has finished. One
-- float for all of them rather than one each, because two questions at once
-- should read as a list, not as windows piled on each other.

local M = {}

M.title = " svara "

-- Wide enough to read a question by, narrow enough to stay in the corner.
local max_width = 60

local spinner = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }

-- What is under way, oldest first: `{ label, started }`.
local requests = {}
local window, buffer, timer
local frame = 0

local function close()
  if timer then
    timer:stop()
    timer:close()
    timer = nil
  end
  if window and vim.api.nvim_win_is_valid(window) then
    vim.api.nvim_win_close(window, true)
  end
  window = nil
  if buffer and vim.api.nvim_buf_is_valid(buffer) then
    vim.api.nvim_buf_delete(buffer, { force = true })
  end
  buffer = nil
end

--- One request as its line: the spinner, the label, and the seconds so far.
---@param request { label: string, started: integer }
---@param now integer
---@param glyph string
---@return string
function M.line(request, now, glyph)
  local seconds = math.floor((now - request.started) / 1000)
  local elapsed = seconds > 0 and (" " .. seconds .. "s") or ""
  local room = max_width - vim.fn.strdisplaywidth(glyph .. "  " .. elapsed)
  local label = request.label:gsub("%s+", " ")
  if vim.fn.strdisplaywidth(label) > room then
    label = vim.fn.strcharpart(label, 0, room - 1) .. "…"
  end
  return string.format("%s %s%s", glyph, label, elapsed)
end

local function draw()
  if #requests == 0 then
    close()
    return
  end
  frame = frame % #spinner + 1
  local now = (vim.uv or vim.loop).now()
  local lines, width = {}, vim.fn.strdisplaywidth(M.title)
  for _, request in ipairs(requests) do
    local line = M.line(request, now, spinner[frame])
    lines[#lines + 1] = line
    width = math.max(width, vim.fn.strdisplaywidth(line))
  end

  if not (buffer and vim.api.nvim_buf_is_valid(buffer)) then
    buffer = vim.api.nvim_create_buf(false, true)
    vim.bo[buffer].bufhidden = "wipe"
  end
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)

  local config = {
    relative = "editor",
    anchor = "NE",
    row = 1,
    col = vim.o.columns,
    width = width,
    height = #lines,
  }
  if window and vim.api.nvim_win_is_valid(window) then
    vim.api.nvim_win_set_config(window, config)
  else
    config.style = "minimal"
    config.border = "rounded"
    config.title = { { M.title, "FloatTitle" } }
    config.title_pos = "left"
    config.focusable = false
    config.noautocmd = true
    config.zindex = 60
    window = vim.api.nvim_open_win(buffer, false, config)
    vim.wo[window].winhighlight = "Normal:NormalFloat"
  end
end

--- Show a request as under way until the returned function is called.
---
--- Calling it more than once is harmless, so a request with several ways to
--- end can call it from each.
---@param label string what is being waited for, as the operator would say it
---@return fun() finished
function M.add(label)
  local request = { label = label, started = (vim.uv or vim.loop).now() }
  requests[#requests + 1] = request
  draw()
  if not timer then
    timer = assert((vim.uv or vim.loop).new_timer())
    timer:start(80, 80, vim.schedule_wrap(function()
      if timer then
        draw()
      end
    end))
  end
  return function()
    for index, candidate in ipairs(requests) do
      if candidate == request then
        table.remove(requests, index)
        break
      end
    end
    draw()
  end
end

--- What is under way, by label, oldest first. For tests and statuslines.
---@return string[]
function M.labels()
  local labels = {}
  for _, request in ipairs(requests) do
    labels[#labels + 1] = request.label
  end
  return labels
end

--- The float, while there is one.
---@return integer? window
function M.window()
  if window and vim.api.nvim_win_is_valid(window) then
    return window
  end
end

return M
