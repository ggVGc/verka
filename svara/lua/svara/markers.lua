-- Markers in the code where a request was made, for as long as it matters.
--
-- `svara.pending`'s float says that something is running, but not where it
-- was asked from — and an edit asked for at one function, then three more
-- asked for elsewhere, are four requests the operator has to remember the
-- places of. So each request from `:SvaraAsk` or `:SvaraEdit` also leaves a
-- line of virtual text at the end of the line it was made on: what was asked,
-- and that it is still being worked on.
--
-- When the request finishes the marker stays, and says so — answered, edited,
-- or failed — since the operator is usually somewhere else by then and the
-- marker is how they find their way back to it. It goes when it is
-- acknowledged (`:SvaraAck`), not on its own. `:SvaraView` opens what the
-- interaction answered: the request, what the agent said, and the places it
-- named, each of which Enter goes to.
--
-- A request can be followed up from that view (`f`), in the same interaction.
-- The follow-up is another turn on the same marker rather than a marker of
-- its own: it is the same conversation about the same place. The marker goes
-- back to waiting — `following up` — and the view shows every turn in order.
--
-- Markers are extmarks, so they follow the line they are on as the buffer is
-- edited around them — an edit landing above one moves it with its line.

local M = {}

M.namespace = vim.api.nvim_create_namespace("svara_markers")

-- Every marker still placed, oldest first.
local markers = {}

local states = {
  waiting = { glyph = "⧗", hl = "DiagnosticInfo" },
  done = { glyph = "✓", hl = "DiagnosticOk" },
  failed = { glyph = "✗", hl = "DiagnosticError" },
}

-- Wide enough to recognise a request by, short enough not to bury the line.
local max_label = 50

--- A marker's virtual text: its state, what it was, and how to act on it once
--- there is something to act on.
---@param marker table
---@return string
function M.text(marker)
  local turn = marker.turns[#marker.turns]
  local label = turn.request:gsub("%s+", " ")
  if vim.fn.strdisplaywidth(label) > max_label then
    label = vim.fn.strcharpart(label, 0, max_label - 1) .. "…"
  end
  local said
  if marker.state == "failed" then
    said = "failed"
  elseif marker.state == "done" then
    said = marker.kind == "edit" and "edited" or "answered"
  elseif #marker.turns > 1 then
    said = "following up"
  else
    said = marker.kind == "edit" and "editing" or "asking"
  end
  local text = string.format("%s svara %s: %s", states[marker.state].glyph, said, label)
  if #marker.turns > 1 then
    text = text .. string.format(" · %d turns", #marker.turns)
  end
  if marker.state ~= "waiting" then
    text = text .. "  (:SvaraView · :SvaraAck)"
  end
  return text
end

local function draw(marker)
  if not vim.api.nvim_buf_is_valid(marker.buffer) then
    return
  end
  local row = marker.extmark and M.row(marker) or marker.row
  if not row then
    return
  end
  marker.extmark = vim.api.nvim_buf_set_extmark(marker.buffer, M.namespace, row, 0, {
    id = marker.extmark,
    virt_text = { { M.text(marker), states[marker.state].hl } },
    virt_text_pos = "eol",
    hl_mode = "combine",
  })
end

--- The 0-based line a marker is on now, wherever edits have moved it.
---@param marker table
---@return integer? row
function M.row(marker)
  if not (marker.extmark and vim.api.nvim_buf_is_valid(marker.buffer)) then
    return nil
  end
  local position = vim.api.nvim_buf_get_extmark_by_id(marker.buffer, M.namespace, marker.extmark, {})
  return position[1]
end

--- Put a marker for `request` at line `line` (1-based) of `buffer`.
---
--- The returned marker is finished with `M.finish`, and has a turn added by
--- `M.follow`; each turn is `{ request, result }`. A buffer with no file —
--- or one already gone — gets no marker, and `nil` is returned; finishing
--- `nil` is harmless, so a caller need not check.
---@param buffer integer
---@param line integer
---@param request string what was asked, as the operator typed it
---@param kind "ask"|"edit"
---@return table? marker
function M.place(buffer, line, request, kind)
  if not vim.api.nvim_buf_is_valid(buffer) or vim.api.nvim_buf_get_name(buffer) == "" then
    return nil
  end
  local last = vim.api.nvim_buf_line_count(buffer)
  local marker = {
    buffer = buffer,
    row = math.max(0, math.min(line, last) - 1),
    turns = { { request = request } },
    kind = kind,
    state = "waiting",
  }
  draw(marker)
  markers[#markers + 1] = marker
  return marker
end

--- Say a marker's request has finished, and keep what it answered to view.
---
--- `result.items` are the places, as quickfix items; `result.error` is why
--- there are none; `result.answer` is the `Answer`, whose `source` is what the
--- agent said either way.
---@param marker table?
---@param result { items?: table[], error?: string, answer?: table }
function M.finish(marker, result)
  if not marker then
    return
  end
  marker.turns[#marker.turns].result = result
  marker.state = result.error and "failed" or "done"
  draw(marker)
end

--- Start another turn on a marker: a follow-up to what it asked, waiting
--- until `M.finish` again.
---@param marker table
---@param request string
function M.follow(marker, request)
  marker.turns[#marker.turns + 1] = { request = request }
  marker.state = "waiting"
  draw(marker)
end

--- Take back the turn `M.follow` added, for a follow-up that never started,
--- and say again how the turn before it went.
---@param marker table
function M.unfollow(marker)
  if #marker.turns < 2 then
    return
  end
  marker.turns[#marker.turns] = nil
  local result = marker.turns[#marker.turns].result
  marker.state = (result and (result.error and "failed" or "done")) or "waiting"
  draw(marker)
end

--- Take a marker away.
---@param marker table
function M.remove(marker)
  for index, candidate in ipairs(markers) do
    if candidate == marker then
      table.remove(markers, index)
      break
    end
  end
  if marker.extmark and vim.api.nvim_buf_is_valid(marker.buffer) then
    pcall(vim.api.nvim_buf_del_extmark, marker.buffer, M.namespace, marker.extmark)
  end
end

--- Every marker still placed, oldest first.
---@return table[]
function M.all()
  return vim.list_slice(markers)
end

--- The newest marker on the cursor's line, or failing that the newest in the
--- buffer, so a command run anywhere in a file with one marker finds it.
---@param finished_only? boolean only markers whose request has finished
---@return table? marker
function M.near_cursor(finished_only)
  local buffer = vim.api.nvim_get_current_buf()
  local row = vim.api.nvim_win_get_cursor(0)[1] - 1
  local in_buffer
  for index = #markers, 1, -1 do
    local marker = markers[index]
    if marker.buffer == buffer and not (finished_only and marker.state == "waiting") then
      if M.row(marker) == row then
        return marker
      end
      in_buffer = in_buffer or marker
    end
  end
  return in_buffer
end

--- What a marker's request answered, as the lines `M.view` shows, and the
--- quickfix item each line goes to, by line number.
---@param marker table
---@return string[] lines
---@return table<integer, table> places
function M.view_lines(marker)
  local lines, places = {}, {}
  local function add(text)
    for line in (text .. "\n"):gmatch("([^\n]*)\n") do
      lines[#lines + 1] = line
    end
  end
  for index, turn in ipairs(marker.turns) do
    if index > 1 then
      add("")
      add("────")
      add("Follow-up: " .. turn.request)
    else
      add((marker.kind == "edit" and "Edit: " or "Asked: ") .. turn.request)
    end
    local result = turn.result
    if not result then
      add("")
      add("Still " .. (index > 1 and "following up" or marker.kind == "edit" and "editing" or "asking") .. "…")
    else
      if result.error then
        add("")
        add("Failed: " .. result.error)
      end
      local source = result.answer and require("svara.api").given(result.answer.source)
      if type(source) == "string" and source ~= "" then
        add("")
        add(source)
      end
      local items = result.items or {}
      if #items > 0 then
        add("")
        add(marker.kind == "edit" and "Changed:" or "Places:")
        for _, item in ipairs(items) do
          local where = vim.fn.fnamemodify(item.filename, ":~:.") .. (item.lnum and (":" .. item.lnum) or "")
          local text = item.text ~= "" and (" — " .. item.text:gsub("%s+", " ")) or ""
          lines[#lines + 1] = "  " .. where .. text
          places[#lines] = item
        end
      end
    end
  end
  return lines, places
end

-- The line the newest turn starts on.
local function last_turn_line(lines)
  for index = #lines, 1, -1 do
    if lines[index]:find("^Follow%-up: ") then
      return index
    end
  end
  return 1
end

--- Open what a marker's request answered in a float over the editor.
---
--- `q` or Esc closes it, `a` acknowledges the marker and closes it, and Enter
--- on one of the places goes there. `f` closes it and follows the request up,
--- when it has finished and whoever placed the marker gave it a
--- `marker.follow_up` to do that with.
---@param marker table
---@return integer window
function M.view(marker)
  local lines, places = M.view_lines(marker)
  local can_follow = marker.follow_up ~= nil and marker.state ~= "waiting"
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.bo[buffer].bufhidden = "wipe"
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
  vim.bo[buffer].modifiable = false
  vim.bo[buffer].filetype = "markdown"

  -- As large as the answer needs, up to nearly the whole editor: an answer is
  -- read once and all the way through, so scrolling it is a cost for nothing.
  -- The height counts wrapped lines, since a paragraph is one line of text.
  local width = math.min(math.max(40, math.floor(vim.o.columns * 0.8)), vim.o.columns - 4)
  local rows = 0
  for _, line in ipairs(lines) do
    rows = rows + math.max(1, math.ceil(vim.fn.strdisplaywidth(line) / width))
  end
  local height = math.max(3, math.min(rows, vim.o.lines - 4))
  local window = vim.api.nvim_open_win(buffer, true, {
    relative = "editor",
    row = math.floor((vim.o.lines - height) / 2) - 1,
    col = math.floor((vim.o.columns - width) / 2),
    width = width,
    height = height,
    style = "minimal",
    border = "rounded",
    title = { { marker.kind == "edit" and " svara edit " or " svara answer ", "FloatTitle" } },
    title_pos = "left",
    footer = { { " q close · a acknowledge · "
      .. (can_follow and "f follow up · " or "") .. "Enter go to place ", "FloatFooter" } },
    footer_pos = "right",
  })
  vim.wo[window].wrap = true
  vim.wo[window].linebreak = true
  -- The newest turn is the one just come back to.
  vim.api.nvim_win_set_cursor(window, { last_turn_line(lines), 0 })

  local function close()
    if vim.api.nvim_win_is_valid(window) then
      vim.api.nvim_win_close(window, true)
    end
  end
  local options = { buffer = buffer, nowait = true }
  vim.keymap.set("n", "q", close, options)
  vim.keymap.set("n", "<Esc>", close, options)
  vim.keymap.set("n", "a", function()
    close()
    M.remove(marker)
  end, options)
  vim.keymap.set("n", "f", function()
    if not can_follow then
      return
    end
    close()
    marker.follow_up(marker)
  end, options)
  vim.keymap.set("n", "<CR>", function()
    local item = places[vim.api.nvim_win_get_cursor(window)[1]]
    if not item then
      return
    end
    close()
    -- A buffer already showing the file is gone to rather than edited again,
    -- which would refuse one with changes not yet written.
    local loaded = vim.fn.bufnr(item.filename)
    if loaded > 0 then
      vim.api.nvim_win_set_buf(0, loaded)
    else
      vim.cmd.edit(vim.fn.fnameescape(item.filename))
    end
    if item.lnum then
      local last = vim.api.nvim_buf_line_count(0)
      vim.api.nvim_win_set_cursor(0, { math.min(item.lnum, last), math.max((item.col or 1) - 1, 0) })
    end
  end, options)
  return window
end

return M
