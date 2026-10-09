-- The window `:SvaraNew` takes its prompt in.
--
-- Driven with keys, as an operator would: what each key does in which mode
-- is the whole of what this window promises, and Enter meaning a newline in
-- one mode and "send" in the other is the part worth pinning. It is Styra's
-- message box, so how it is drawn — the model in the border, the placeholder,
-- the height — is pinned too.
--
--   nvim --headless -u NONE -l tests/compose_spec.lua

local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

local compose = require("svara.compose")

local notified = {}
vim.notify = function(message)
  notified[#notified + 1] = message
end

--- The window opens in Insert mode with `:startinsert`, which takes effect
--- when the command that opened it returns. A script run with `-l` never
--- returns while it is testing, so typing here starts with the `i` or `A` the
--- window would have pressed for itself.
---
--- What the window does about a change — the placeholder, its height — is
--- scheduled, so the event loop is given a turn after each lot of keys.
local function keys(typed)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(typed, true, false, true), "x", false)
  vim.wait(10)
end

--- The window's top border, as the text it shows.
local function border_title(window)
  local text = {}
  for _, chunk in ipairs(vim.api.nvim_win_get_config(window).title) do
    text[#text + 1] = chunk[1]
  end
  return table.concat(text)
end

local function shown_lines()
  return vim.api.nvim_buf_get_lines(0, 0, -1, false)
end

--- Open the window and return what came of it: the prompt and whether it
--- asked for a Git workspace, or `cancelled`. `choose_model`, when given,
--- is what Ctrl+L runs. `hold` leaves a send under way until the test says
--- how it went, through `result.progress`.
local function open(options)
  options = options or {}
  local result = { editing = vim.api.nvim_get_current_win() }
  result.window, result.buffer = compose.open({
    initial = options.initial,
    model = options.model == nil and "claude-opus-5 · high" or options.model or nil,
    choose_model = options.choose_model,
    title = options.title,
    placeholder = options.placeholder,
    sending = options.sending,
    worktree = options.worktree,
    on_send = function(prompt, create_worktree, progress)
      result.prompt, result.create_worktree = prompt, create_worktree
      result.progress = progress
      result.sends = (result.sends or 0) + 1
      -- Done at once unless the test wants to see the wait.
      if not options.hold then
        progress.done()
      end
    end,
    on_cancel = function()
      result.cancelled = true
    end,
  })
  return result
end

-- Styra's box: the model in the border, a placeholder until typed in -------

do
  local opened = open()
  assert(vim.api.nvim_get_current_win() == opened.window)
  local config = vim.api.nvim_win_get_config(opened.window)
  assert(config.relative == "editor", "a floating window")
  assert(config.width <= 80, tostring(config.width))
  local title = border_title(opened.window)
  assert(vim.startswith(title, " message ─"), title)
  assert(vim.endswith(title, "─ claude-opus-5 · high "), title)
  assert(vim.fn.strdisplaywidth(title) == config.width, "the title spans the border")

  local marks = vim.api.nvim_buf_get_extmarks(opened.buffer, -1, 0, -1, { details = true })
  assert(#marks == 1 and marks[1][4].virt_text[1][1] == compose.placeholder, vim.inspect(marks))
  keys("ihello<Esc>")
  marks = vim.api.nvim_buf_get_extmarks(opened.buffer, -1, 0, -1, {})
  assert(#marks == 0, "the placeholder goes once there is text")
  keys("q")
end

-- Growing with what is typed, as far as the screen allows ----------------

do
  local opened = open()
  assert(vim.api.nvim_win_get_config(opened.window).height == 1)
  keys("ione<CR>two<CR>three<Esc>")
  assert(vim.api.nvim_win_get_config(opened.window).height == 3)
  keys("o4<CR>5<CR>6<CR>7<CR>8<Esc>")
  assert(vim.api.nvim_win_get_config(opened.window).height == 8, "no height of its own to stop at")
  -- Only a prompt taller than the screen scrolls.
  keys(string.rep("oline<Esc>", vim.o.lines))
  local room = vim.o.lines - vim.o.cmdheight - 2
  assert(vim.api.nvim_win_get_config(opened.window).height == room)
  keys("q")
end

-- Enter is a newline in Insert mode and sends from Normal mode -----------

do
  local opened = open()
  keys("ifirst line<CR>second line<Esc>")
  assert(opened.prompt == nil, "nothing sent while editing")
  assert(vim.api.nvim_win_is_valid(opened.window))
  -- Normal mode is Vim's own: `dd` on the second line, then Enter to send.
  keys("dd")
  keys("<CR>")
  assert(opened.prompt == "first line", vim.inspect(opened.prompt))
  assert(opened.create_worktree == false)
  assert(not vim.api.nvim_win_is_valid(opened.window), "sending closes the window")
  assert(vim.api.nvim_get_current_win() == opened.editing)
end

-- Ctrl+Enter, from either mode --------------------------------------------

do
  local opened = open()
  keys("ibranch this<C-CR>")
  assert(opened.prompt == "branch this", vim.inspect(opened.prompt))
  assert(opened.create_worktree == true)
  assert(vim.fn.mode() == "n", "Insert mode is left with the window")

  opened = open()
  keys("iand this<Esc><C-CR>")
  assert(opened.prompt == "and this", vim.inspect(opened.prompt))
  assert(opened.create_worktree == true)
end

-- The command line's words are where the prompt starts -------------------

do
  local opened = open({ initial = "why does" })
  keys("A this fail?<Esc><CR>")
  assert(opened.prompt == "why does this fail?", vim.inspect(opened.prompt))
end

-- An empty prompt is not sent --------------------------------------------

do
  local opened = open()
  keys("i   <Esc><CR>")
  assert(opened.prompt == nil)
  assert(vim.api.nvim_win_is_valid(opened.window), "the window stays to be written in")
  assert(notified[#notified]:find("needs a prompt"), notified[#notified])
  keys("q")
  assert(opened.cancelled)
end

-- Closing it any other way is cancelling ---------------------------------

do
  -- Escape from Insert mode only leaves it; from Normal mode it closes.
  local opened = open()
  keys("ihalf a thought<Esc>")
  assert(vim.api.nvim_win_is_valid(opened.window), "the first Escape leaves Insert mode")
  assert(not opened.cancelled)
  keys("<Esc>")
  assert(opened.cancelled, "the second closes the window")
  assert(not vim.api.nvim_win_is_valid(opened.window))
  assert(opened.prompt == nil)

  opened = open({ initial = "half a thought" })
  vim.cmd.close()
  vim.wait(100, function()
    return opened.cancelled
  end)
  assert(opened.cancelled, "`:close` says nothing was sent")
  assert(opened.prompt == nil)

  opened = open({ initial = "elsewhere" })
  vim.api.nvim_set_current_win(opened.editing)
  vim.wait(100, function()
    return opened.cancelled
  end)
  assert(opened.cancelled, "leaving the window closes it")
  assert(not vim.api.nvim_win_is_valid(opened.window))
end

-- Ctrl+L: the configured picker, over the window ---------------------------

--- A picker as Telescope and its like are: a window of its own, taking the
--- focus, answered later. `picked.answer(label)` closes it and gives the
--- answer, `nil` for backing out.
local function picker(picked)
  return function(done)
    picked.count = (picked.count or 0) + 1
    local buffer = vim.api.nvim_create_buf(false, true)
    local window = vim.api.nvim_open_win(buffer, true, {
      relative = "editor",
      width = 20,
      height = 3,
      row = 1,
      col = 1,
    })
    picked.window = window
    picked.answer = function(label)
      vim.api.nvim_win_close(window, true)
      done(label)
      vim.wait(20)
    end
  end
end

do
  local picked = {}
  local opened = open({ choose_model = picker(picked) })
  keys("ia prompt<C-l>")
  assert(picked.count == 1)
  vim.wait(20)
  assert(vim.api.nvim_get_current_win() == picked.window, "the picker has the focus")
  assert(vim.api.nvim_win_is_valid(opened.window), "the window stays open under the picker")
  assert(not opened.cancelled, "leaving for the picker is not closing")

  -- The answer goes back to the prompt as it was, naming the new model.
  picked.answer("claude-haiku · high")
  assert(vim.api.nvim_get_current_win() == opened.window, "back at the prompt")
  assert(vim.deep_equal(shown_lines(), { "a prompt" }), vim.inspect(shown_lines()))
  local title = border_title(opened.window)
  assert(title:find(" claude%-haiku · high $"), title)
  assert(opened.prompt == nil, "choosing a model sends nothing")

  -- Ctrl+L was pressed in Insert mode, and the prompt is given back in it —
  -- once a command returns, which a script's never does; see `keys`.
  if vim.fn.mode() == "i" then
    keys("<Esc>")
  end
  keys("<CR>")
  assert(opened.prompt == "a prompt", vim.inspect(opened.prompt))
end

-- Backing out of the picker goes back to the prompt, model unchanged ------

do
  local picked = {}
  local opened = open({ choose_model = picker(picked) })
  keys("ikeep me<Esc><C-l>")
  picked.answer(nil)
  assert(vim.api.nvim_win_is_valid(opened.window))
  assert(vim.api.nvim_get_current_win() == opened.window)
  assert(border_title(opened.window):find(" claude%-opus%-5 · high $"))
  assert(not opened.cancelled)

  -- Leaving the window once the picker is done is closing it again.
  vim.api.nvim_set_current_win(opened.editing)
  vim.wait(100, function()
    return opened.cancelled
  end)
  assert(opened.cancelled, "leaving after the picker still closes")
end

-- With no model to name, sending asks for one first ----------------------

do
  local picked = {}
  local opened = open({ model = false, choose_model = picker(picked) })
  local title = border_title(opened.window)
  assert(title:find(compose.no_model, 1, true), title)
  keys("ifind it<C-CR>")
  assert(picked.count == 1, "the picker is the answer to sending without a model")
  assert(opened.prompt == nil)
  picked.answer("claude-opus-5 · high")
  assert(opened.prompt == "find it", vim.inspect(opened.prompt))
  assert(opened.create_worktree == true, "sent the way it was asked to be")

  -- Backing out instead leaves the prompt unsent and the window open.
  picked = {}
  opened = open({ model = false, choose_model = picker(picked) })
  keys("ifind it<Esc><CR>")
  picked.answer(nil)
  assert(opened.prompt == nil)
  assert(vim.api.nvim_get_current_buf() == opened.buffer)
  keys("q")
end

-- A send under way is shown, not frozen on --------------------------------

local function bottom_border(window)
  local text = {}
  for _, chunk in ipairs(vim.api.nvim_win_get_config(window).footer or {}) do
    text[#text + 1] = chunk[1]
  end
  return table.concat(text)
end

do
  local opened = open({ hold = true })
  keys("ibranch it<C-CR>")
  assert(opened.sends == 1)
  assert(vim.api.nvim_win_is_valid(opened.window), "the window waits with the operator")
  assert(bottom_border(opened.window):find(compose.branching, 1, true), bottom_border(opened.window))
  assert(not vim.bo[opened.buffer].modifiable, "the prompt is not edited under a send")
  -- The spinner turns while the editor carries on.
  local before = bottom_border(opened.window)
  vim.wait(200)
  assert(bottom_border(opened.window) ~= before, "the spinner moves")

  -- A second send while one is under way is not a second start.
  keys("<CR><C-CR>")
  assert(opened.sends == 1)

  -- Failing gives the prompt back, saying why.
  opened.progress.failed("not a Git repository")
  assert(vim.api.nvim_win_is_valid(opened.window))
  assert(vim.bo[opened.buffer].modifiable)
  assert(bottom_border(opened.window):find("not a Git repository", 1, true))
  local spun = bottom_border(opened.window)
  vim.wait(200)
  assert(bottom_border(opened.window) == spun, "the spinner has stopped")

  -- And it can be sent again, this time without a Git workspace.
  keys("<CR>")
  assert(opened.sends == 2 and opened.create_worktree == false)
  assert(bottom_border(opened.window):find(compose.starting, 1, true), bottom_border(opened.window))
  opened.progress.done()
  assert(not vim.api.nvim_win_is_valid(opened.window), "done closes the window")
end

-- Closing it while a send is under way is not cancelling the send ---------

do
  local opened = open({ hold = true })
  keys("ilong one<C-CR>")
  keys("q")
  assert(not vim.api.nvim_win_is_valid(opened.window))
  assert(not opened.cancelled, "the start goes on")
  assert(notified[#notified]:find("still starting"), notified[#notified])
  -- Its end still arrives, and finds nothing left to show it in.
  opened.progress.done()
  opened.progress.failed("too late to show")
end

-- `:SvaraAsk`'s box: its own words, and no Git workspace ------------------

do
  local opened = open({
    hold = true,
    title = " question ",
    placeholder = "Enter to ask",
    sending = "asking…",
    worktree = false,
  })
  local title = border_title(opened.window)
  assert(vim.startswith(title, " question ─"), title)
  local marks = vim.api.nvim_buf_get_extmarks(opened.buffer, -1, 0, -1, { details = true })
  assert(marks[1][4].virt_text[1][1] == "Enter to ask", vim.inspect(marks))

  -- Ctrl+Enter is no send here, from either mode. (`keys` leaves Insert
  -- mode once its keys are typed, so the second is from Normal mode.)
  keys("iwhere is auth?<C-CR>")
  assert(opened.sends == nil, "Ctrl+Enter sent a question")
  keys("<C-CR>")
  assert(opened.sends == nil, "Ctrl+Enter sent a question")
  assert(vim.api.nvim_win_is_valid(opened.window))

  keys("<CR>")
  assert(opened.sends == 1 and opened.create_worktree == false)
  assert(opened.prompt == "where is auth?", vim.inspect(opened.prompt))
  assert(bottom_border(opened.window):find("asking…", 1, true), bottom_border(opened.window))
  opened.progress.done()
  assert(not vim.api.nvim_win_is_valid(opened.window))
end

print("svara compose tests passed")
