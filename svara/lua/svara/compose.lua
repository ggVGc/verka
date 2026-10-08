-- The window `:SvaraNew` takes its prompt in: Styra's message box, in Neovim.
--
-- A command line is one line long and has no modes, and a first prompt is
-- often a paragraph worth editing before it goes. So the prompt is written in
-- an ordinary buffer in a floating window, with the whole of Vim to write it
-- with, and the window is drawn and driven the way Styra's own message box
-- is: ` message ` at the top left of its border, the model it will start on at
-- the top right, a placeholder saying how to send while it is empty, centred,
-- at most 80 columns wide, and growing with what is typed. Unlike Styra's it
-- has no height of its own to stop at: all of the prompt stays in sight, and
-- only a prompt taller than the screen scrolls.
--
-- Enter sends, starting in the Workspace directory; Ctrl+Enter sends starting
-- in a new Git workspace and branch; Ctrl+L chooses the model. Enter only sends
-- from Normal mode — in Insert mode it is a newline, as in every other buffer,
-- and leaving Insert mode is the step that says the prompt is finished. The
-- other two mean nothing else, so work from either.
--
-- The model picker is shown in the same window, one step replacing the
-- prompt and handing back to it. The window offers it as `select` and `input`
-- with `vim.ui`'s signatures, so the code doing the asking does not need to
-- know where it is being asked. Each step is a buffer of its own shown in the
-- one window, so each has its own keys and the prompt is still there to come
-- back to.

local M = {}

M.title = " message "
M.placeholder = "Enter to send · Ctrl+Enter to send in a new Git workspace"
M.no_model = "no model · Ctrl+L"
M.starting = "starting…"
M.branching = "creating a Git workspace and branch…"
M.list_hint = " Enter choose · Esc back "
M.input_hint = " Enter confirm · Esc back "

-- Styra's box: as wide as prose wants, and at least one line of text.
local max_width = 80
local min_lines = 1
local list_lines = 15

local spinner = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }

local placeholder_namespace = vim.api.nvim_create_namespace("svara_compose_placeholder")

--- The top border as title chunks: ` message ` at the left, the model at the
--- right, and border between, since a float has one title and Styra's box has
--- two.
---@param width integer the window's inner width
---@param model? string
function M.title_chunks(width, model)
  local right = " " .. (model or M.no_model) .. " "
  local fill = width - vim.fn.strdisplaywidth(M.title) - vim.fn.strdisplaywidth(right)
  return {
    { M.title, "Comment" },
    { string.rep("─", math.max(fill, 1)), "FloatBorder" },
    { right, model and "FloatTitle" or "WarningMsg" },
  }
end

--- How many screen lines `lines` take at `width` columns, wrapped.
local function wrapped_height(lines, width)
  local rows = 0
  for _, line in ipairs(lines) do
    rows = rows + math.max(1, math.ceil(vim.fn.strdisplaywidth(line) / width))
  end
  return rows
end

--- Open the window. `on_send(prompt, create_worktree, progress)` is called
--- once the operator sends from it, and `on_cancel()` if they close it unsent.
---
--- Sending does not close the window: starting can take a while — making a
--- Git workspace and naming its branch is seconds of the server's time — and
--- a box that vanished on Enter would leave nothing to say so. Instead it
--- stays, read-only, with a spinner and what is happening in its bottom
--- border, until `on_send` calls `progress.done()`, which closes it, or
--- `progress.failed(message)`, which gives the prompt back to be edited with
--- the message under it, as Styra does with a launch that fails. Closing it
--- meanwhile only stops showing the wait; the start goes on regardless.
---
--- `model` is what the border names, nil when there is nothing to name yet.
--- `choose_model(ui, done)` is what Ctrl+L runs: it asks through `ui` —
--- `{ select, input }`, shaped like `vim.ui`'s and shown in this window — and
--- calls `done(label)` with the new model to name, or `done(nil)` if the
--- operator backed out. Sending with no model runs it first, and sends once it
--- has one.
---
--- `initial` is put in the buffer to start from, so `:SvaraNew some words`
--- still means something: the beginning of the prompt rather than all of it.
--- An empty prompt is not sent; the window stays open and says why.
---@param options { initial?: string, model?: string, choose_model?: fun(ui: table, done: fun(label?: string)), on_send: fun(prompt: string, create_worktree: boolean, progress: { done: fun(), failed: fun(message: string) }), on_cancel?: fun() }
---@return integer window
---@return integer buffer the prompt's
function M.open(options)
  local model = options.model
  local prompt_buffer = vim.api.nvim_create_buf(false, true)
  -- Hidden rather than wiped while the picker is shown over it, so it is
  -- found as it was left. Wiped by hand when the window goes.
  vim.bo[prompt_buffer].bufhidden = "hide"
  vim.bo[prompt_buffer].filetype = "markdown"
  local initial = options.initial or ""
  vim.api.nvim_buf_set_lines(prompt_buffer, 0, -1, false, vim.split(initial, "\n", { plain = true }))

  local width = math.max(20, math.min(max_width, vim.o.columns - 4))

  --- Centred, as tall as `rows` lines of text.
  local function placement(rows)
    return {
      relative = "editor",
      width = width,
      height = rows,
      row = math.max(0, math.floor((vim.o.lines - rows - 2) / 2)),
      col = math.floor((vim.o.columns - width - 2) / 2),
    }
  end

  local function prompt_rows()
    local lines = vim.api.nvim_buf_get_lines(prompt_buffer, 0, -1, false)
    -- The screen is the only limit: its height, less the border and the
    -- command line.
    local room = math.max(min_lines, vim.o.lines - vim.o.cmdheight - 2)
    return math.max(min_lines, math.min(room, wrapped_height(lines, width)))
  end

  local config = placement(prompt_rows())
  config.style = "minimal"
  config.border = "rounded"
  config.title = M.title_chunks(width, model)
  config.title_pos = "left"
  local window = vim.api.nvim_open_win(prompt_buffer, true, config)
  vim.wo[window].wrap = true
  vim.wo[window].linebreak = true

  local group = vim.api.nvim_create_augroup("svara_compose_" .. window, { clear = true })
  local finished = false
  local on_prompt = true
  -- Counts the steps shown, so an answer that asked nothing more can be told
  -- from one that did: the first goes back to the prompt, the second stays.
  local steps = 0
  -- A send waiting on a model to send with: whether it was Ctrl+Enter's.
  local waiting_to_send = nil
  -- Where the cursor was, and in which mode, when the prompt was left.
  local left_at, left_inserting
  -- The timer turning the spinner while a send is under way, or nil.
  local busy

  local function placeholder()
    vim.api.nvim_buf_clear_namespace(prompt_buffer, placeholder_namespace, 0, -1)
    local lines = vim.api.nvim_buf_get_lines(prompt_buffer, 0, -1, false)
    if #lines == 1 and lines[1] == "" then
      vim.api.nvim_buf_set_extmark(prompt_buffer, placeholder_namespace, 0, 0, {
        virt_text = { { M.placeholder, "Comment" } },
        virt_text_pos = "overlay",
      })
    end
  end

  local function fit()
    if on_prompt and vim.api.nvim_win_is_valid(window) then
      vim.api.nvim_win_set_config(window, placement(prompt_rows()))
    end
  end

  local function close()
    if finished then
      return
    end
    finished = true
    if busy then
      busy:stop()
      busy:close()
      busy = nil
    end
    pcall(vim.api.nvim_del_augroup_by_id, group)
    if vim.api.nvim_win_is_valid(window) then
      vim.api.nvim_win_close(window, true)
    end
    if vim.api.nvim_buf_is_valid(prompt_buffer) then
      vim.api.nvim_buf_delete(prompt_buffer, { force = true })
    end
  end

  local function footer(chunks)
    if vim.api.nvim_win_is_valid(window) then
      vim.api.nvim_win_set_config(window, { footer = chunks, footer_pos = "left" })
    end
  end

  local function stop_spinning()
    if busy then
      busy:stop()
      busy:close()
      busy = nil
    end
  end

  --- Show the wait in the bottom border, with how long it has gone on.
  local function spin(message)
    vim.bo[prompt_buffer].modifiable = false
    local started, frame = vim.uv.now(), 0
    local function draw()
      frame = frame % #spinner + 1
      local seconds = math.floor((vim.uv.now() - started) / 1000)
      local text = string.format(" %s %s ", spinner[frame], message)
      if seconds > 0 then
        text = text .. seconds .. "s "
      end
      footer({ { text, "WarningMsg" } })
    end
    draw()
    busy = assert(vim.uv.new_timer())
    busy:start(80, 80, vim.schedule_wrap(function()
      if busy and not finished then
        draw()
      end
    end))
  end

  local function cancel()
    if finished then
      return
    end
    if busy then
      -- The start is the server's now, and goes on without the window.
      stop_spinning()
      close()
      vim.notify("Svara: still starting; this will say when it has", vim.log.levels.INFO)
      return
    end
    close()
    if options.on_cancel then
      options.on_cancel()
    end
  end

  local function map(buffer, modes, lhs, action, desc)
    vim.keymap.set(modes, lhs, action, { buffer = buffer, nowait = true, desc = desc })
  end

  local send

  --- Back to the prompt, where and as it was left.
  local function show_prompt()
    steps = steps + 1
    on_prompt = true
    waiting_to_send = nil
    vim.api.nvim_win_set_buf(window, prompt_buffer)
    local config = placement(prompt_rows())
    config.title = M.title_chunks(width, model)
    config.title_pos = "left"
    config.footer = ""
    vim.api.nvim_win_set_config(window, config)
    vim.wo[window].cursorline = false
    if left_at then
      pcall(vim.api.nvim_win_set_cursor, window, left_at)
    end
    if left_inserting then
      local line = vim.api.nvim_buf_get_lines(prompt_buffer, left_at[1] - 1, left_at[1], false)[1]
      vim.cmd.startinsert({ bang = left_at[2] >= #line })
    end
  end

  --- Run an answer, then go back to the prompt unless it asked something more.
  local function answer(callback, ...)
    local before = steps
    callback(...)
    if not finished and steps == before then
      show_prompt()
    end
  end

  local function show_step(buffer, title, hint, rows)
    steps = steps + 1
    on_prompt = false
    vim.api.nvim_win_set_buf(window, buffer)
    local config = placement(math.max(min_lines, math.min(list_lines, rows)))
    config.title = title
    config.title_pos = "center"
    config.footer = hint
    config.footer_pos = "center"
    vim.api.nvim_win_set_config(window, config)
  end

  local ui = {}

  --- A list, one item to a line, chosen with the cursor.
  function ui.select(items, opts, on_choice)
    opts = opts or {}
    local format = opts.format_item or tostring
    local lines = {}
    for index, item in ipairs(items) do
      lines[index] = format(item)
    end
    local buffer = vim.api.nvim_create_buf(false, true)
    vim.bo[buffer].bufhidden = "wipe"
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
    vim.bo[buffer].modifiable = false
    local title = opts.prompt and (" " .. opts.prompt .. " ") or M.title
    show_step(buffer, title, M.list_hint, #lines)
    vim.wo[window].cursorline = true
    vim.api.nvim_win_set_cursor(window, { 1, 0 })

    map(buffer, "n", "<CR>", function()
      local index = vim.api.nvim_win_get_cursor(window)[1]
      answer(on_choice, items[index], index)
    end, "Choose this one")
    local function back()
      answer(on_choice, nil, nil)
    end
    map(buffer, "n", "q", back, "Back to the prompt")
    map(buffer, "n", "<Esc>", back, "Back to the prompt")
    map(buffer, "n", "<BS>", back, "Back to the prompt")
  end

  --- One line to type in, confirmed with Enter from either mode: there is no
  --- second line for Enter to make.
  function ui.input(opts, on_confirm)
    opts = opts or {}
    local buffer = vim.api.nvim_create_buf(false, true)
    vim.bo[buffer].bufhidden = "wipe"
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { opts.default or "" })
    local title = opts.prompt and (" " .. vim.trim(opts.prompt) .. " ") or M.title
    show_step(buffer, title, M.input_hint, 1)
    vim.wo[window].cursorline = false
    vim.api.nvim_win_set_cursor(window, { 1, #(opts.default or "") })

    map(buffer, { "n", "i" }, "<CR>", function()
      vim.cmd.stopinsert()
      answer(on_confirm, vim.api.nvim_buf_get_lines(buffer, 0, 1, false)[1])
    end, "Confirm")
    local function back()
      answer(on_confirm, nil)
    end
    map(buffer, "n", "q", back, "Back to the prompt")
    map(buffer, "n", "<Esc>", back, "Back to the prompt")
    vim.cmd.startinsert({ bang = true })
  end

  local function choose_model()
    if not options.choose_model or busy then
      return
    end
    left_at = vim.api.nvim_win_get_cursor(window)
    left_inserting = vim.fn.mode() == "i"
    vim.cmd.stopinsert()
    answer(options.choose_model, ui, function(label)
      local resend = waiting_to_send
      waiting_to_send = nil
      if label then
        model = label
      end
      if label and resend ~= nil then
        send(resend)
      elseif not finished then
        show_prompt()
      end
    end)
  end

  local progress = {}

  function progress.done()
    stop_spinning()
    close()
  end

  function progress.failed(message)
    stop_spinning()
    if finished then
      return
    end
    vim.bo[prompt_buffer].modifiable = true
    footer({ { " " .. message .. " ", "ErrorMsg" } })
  end

  function send(create_worktree)
    if busy then
      return
    end
    local prompt =
      vim.trim(table.concat(vim.api.nvim_buf_get_lines(prompt_buffer, 0, -1, false), "\n"))
    if prompt == "" then
      vim.notify("Svara: a new interaction needs a prompt", vim.log.levels.WARN)
      return
    end
    if not model and options.choose_model then
      -- Nothing to start on yet: ask, and send once there is.
      waiting_to_send = create_worktree
      choose_model()
      return
    end
    vim.cmd.stopinsert()
    spin(create_worktree and M.branching or M.starting)
    options.on_send(prompt, create_worktree, progress)
  end

  map(prompt_buffer, "n", "<CR>", function()
    send(false)
  end, "Send the prompt, starting in this Workspace")
  map(prompt_buffer, { "n", "i" }, "<C-CR>", function()
    send(true)
  end, "Send the prompt, starting in a new Git workspace and branch")
  map(prompt_buffer, { "n", "i" }, "<C-l>", choose_model, "Choose the model")
  -- Escape only closes from Normal mode: from Insert mode it is the step
  -- out of typing, as everywhere else.
  map(prompt_buffer, "n", "q", cancel, "Close without sending")
  map(prompt_buffer, "n", "<Esc>", cancel, "Close without sending")

  -- Scheduled, because a buffer may not be redecorated from inside the
  -- change being reported.
  vim.api.nvim_buf_attach(prompt_buffer, false, {
    on_lines = function()
      if finished then
        return true
      end
      vim.schedule(function()
        if not finished then
          placeholder()
          fit()
        end
      end)
    end,
  })
  -- Leaving the window any other way — `:q`, a jump to another window — is
  -- closing it unsent too, and should say so rather than leave a callback
  -- that never comes.
  vim.api.nvim_create_autocmd("WinClosed", {
    group = group,
    pattern = tostring(window),
    callback = function()
      vim.schedule(cancel)
    end,
  })
  vim.api.nvim_create_autocmd("WinLeave", {
    group = group,
    callback = function()
      if vim.api.nvim_get_current_win() == window then
        vim.schedule(cancel)
      end
    end,
  })

  placeholder()
  if initial == "" then
    vim.cmd.startinsert()
  else
    local last = vim.api.nvim_buf_line_count(prompt_buffer)
    local text = vim.api.nvim_buf_get_lines(prompt_buffer, last - 1, last, false)[1]
    vim.api.nvim_win_set_cursor(window, { last, #text })
    vim.cmd.startinsert({ bang = true })
  end
  return window, prompt_buffer
end

return M
