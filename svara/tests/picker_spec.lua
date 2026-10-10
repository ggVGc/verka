-- The model `:SvaraNew` names, and the list Ctrl+L offers to change it.
--
-- The command's own question, which is the one part of Svara an operator
-- answers by hand every time, so what is in the list and in what order is
-- worth pinning: the model already in use first, the server's catalog under
-- it, typing one out last, and a reasoning effort asked for only where the
-- model has a ladder to ask about.
--
-- `svara.core` is stubbed here rather than served. The wiring it stands for —
-- which selection, which catalog — is covered in `tests/info_spec.lua` over a
-- host of its own; this is about what the command does with the answers.
--
--   nvim --headless -u NONE -l tests/picker_spec.lua

local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path
vim.opt.runtimepath:append(root)
vim.cmd("runtime plugin/svara.lua")

local core = require("svara.core")

local notified = {}
vim.notify = function(message)
  notified[#notified + 1] = message
end

local in_use = { provider = "claude", model = "claude-opus-5", effort = "high" }
local catalog = {
  { provider = "codex", model = "gpt-6-astra", efforts = { "low", "high" }, default_effort = "high" },
  { provider = "claude", model = "claude-haiku-4-5-20251001", efforts = {}, default_effort = "high" },
}

core.selection_for_directory = function()
  return in_use, "the newest Session in the Workspace"
end
core.available_models = function()
  return catalog
end

local started, branched
core.start = function(_, options)
  started, branched = options.selection, options.create_worktree
  return { id = "styra-1", selection = require("svara.api").selection(options.selection) }
end

local file = vim.fn.tempname() .. ".lua"
vim.fn.writefile({ "local a = 1" }, file)
vim.cmd.edit(file)

-- The window the prompt is typed in has a spec of its own,
-- `tests/compose_spec.lua`. Here it sends what the command line gave it, with
-- Enter unless a test says Ctrl+Enter — after Ctrl+L, when a test has picks
-- to answer the model lists with, and, as the window does, whenever there is
-- no model to name.
local compose = require("svara.compose")
local ctrl_l, ctrl_enter, window = false, false, nil
compose.open = function(options)
  window = { model = options.model }
  local function send()
    options.on_send(options.initial, ctrl_enter, {
      done = function()
        window.sent = true
      end,
      failed = function(message)
        window.failed = message
      end,
    })
  end
  if ctrl_l or not options.model then
    options.choose_model(function(label)
      window.answered = true
      if label then
        window.model = label
        send()
      end
    end)
  else
    send()
  end
end

--- `:SvaraNew`. With `picks`, Ctrl+L is pressed and its lists are answered
--- with them in turn — an index, or nothing to back out. Ctrl+Enter sends
--- when `branch` is set. Returns what each model list showed, and what the
--- window was told.
local function run(picks, branch)
  local shown, turn = {}, 0
  ctrl_l, ctrl_enter = picks ~= nil, branch == true
  picks = picks or {}
  vim.ui.select = function(items, options, on_choice)
    turn = turn + 1
    local labels = {}
    for _, item in ipairs(items) do
      labels[#labels + 1] = options.format_item and options.format_item(item) or tostring(item)
    end
    shown[turn] = labels
    on_choice(picks[turn] and items[picks[turn]] or nil, picks[turn])
  end
  started, branched, vim.g.svara_new_selection = nil, nil, nil
  vim.cmd("SvaraNew what does this module trust?")
  return shown, window
end

-- The model in use is named in the border, and Enter starts on it ---------

do
  local shown, opened = run()
  assert(#shown == 0, "no list unless Ctrl+L asks for one")
  assert(opened.model == "claude-opus-5 · high", tostring(opened.model))
  assert(started.model == "claude-opus-5", vim.inspect(started))
  assert(vim.g.svara_new_selection == nil, "sending stores nothing")
  assert(opened.sent, "the window is told the start is done")
end

-- One list, the model in use at the top of it -----------------------------

do
  local shown = run({ 1 })
  assert(#shown == 1, "one list, not a confirmation and then a list")
  assert(
    shown[1][1] == "claude:claude-opus-5/high (in use, from the newest Session in the Workspace)",
    shown[1][1]
  )
  assert(shown[1][2] == "codex:gpt-6-astra", vim.inspect(shown[1]))
  assert(shown[1][4] == "another model…", vim.inspect(shown[1]))
  assert(started.model == "claude-opus-5", vim.inspect(started))
  -- Carrying on with what the rules already give changes nothing, so there
  -- is nothing to store.
  assert(vim.g.svara_new_selection == nil, "carrying on stores nothing")
end

-- A model from the catalog, and the rungs it accepts ----------------------

do
  local shown = run({ 2, 2 })
  assert(shown[2][1] == "low", vim.inspect(shown[2]))
  assert(shown[2][2] == "high (default)", vim.inspect(shown[2]))
  assert(started == "codex:gpt-6-astra/high", vim.inspect(started))
  assert(vim.g.svara_new_selection == "codex:gpt-6-astra/high", tostring(vim.g.svara_new_selection))
  -- And the border names it from then on.
  local _, opened = run({ 2, 1 })
  assert(opened.model == "gpt-6-astra · low", tostring(opened.model))
end

-- A model with no ladder is not asked about ------------------------------

do
  local shown = run({ 3 })
  assert(#shown == 1, vim.inspect(shown))
  -- It still needs an effort in its Selection: the one a launch would use.
  assert(
    vim.g.svara_new_selection == "claude:claude-haiku-4-5-20251001/high",
    tostring(vim.g.svara_new_selection)
  )
end

-- Typing one out, for an id newer than the server's tables ----------------

do
  vim.ui.input = function(_, on_input)
    on_input("claude:claude-fable-9/max")
  end
  run({ 4 })
  assert(started == "claude:claude-fable-9/max", vim.inspect(started))
  assert(vim.g.svara_new_selection == "claude:claude-fable-9/max")

  -- An unusable one is refused, and nothing starts.
  vim.ui.input = function(_, on_input)
    on_input("claude")
  end
  run({ 4 })
  assert(started == nil, vim.inspect(started))
  assert(notified[#notified]:find("reasoning effort"), notified[#notified])
end

-- Where to start: Styra's Enter or its Ctrl+Enter -------------------------

do
  run()
  assert(branched == false, "Enter is not a new Git workspace")

  run(nil, true)
  assert(started.model == "claude-opus-5", vim.inspect(started))
  assert(branched == true, "Ctrl+Enter asks for a new Git workspace")
end

-- Backing out of the list -----------------------------------------------

do
  local _, opened = run({})
  assert(opened.answered, "backing out still hands the window back its prompt")
  assert(started == nil, vim.inspect(started))
  assert(opened.model == "claude-opus-5 · high", "the model named is left as it was")
end

-- Nothing in use: the catalog is the whole list ---------------------------

do
  core.selection_for_directory = function()
    return nil, nil, "no Session in Workspace \"verka\" to take a model from"
  end
  local shown, opened = run({ 1, 2 })
  assert(opened.model == "gpt-6-astra · high", "named once one is chosen")
  assert(shown[1][1] == "codex:gpt-6-astra", vim.inspect(shown[1]))
  assert(started == "codex:gpt-6-astra/high", vim.inspect(started))

  -- And with no catalog either, there is no list to show at all.
  core.available_models = function()
    return nil, "could not reach the Styra server"
  end
  vim.ui.input = function(_, on_input)
    on_input("claude:claude-opus-5/xhigh")
  end
  local asked = run({})
  assert(#asked == 0, vim.inspect(asked))
  assert(started == "claude:claude-opus-5/xhigh", vim.inspect(started))
end

-- :SvaraEdit, its own model, and :SvaraJump to where it changed ----------

do
  local asked, finish
  core.selection_for_directory = function(_, options)
    asked = options and options.kind
    return in_use, "the newest Session in the Workspace"
  end
  core.edit = function(instruction, options, on_done)
    finish = { instruction = instruction, options = options, on_done = on_done }
    return {}, nil, { id = "styra-5" }
  end
  vim.fn.writefile({ "one", "two", "three" }, file)
  vim.cmd("edit! " .. vim.fn.fnameescape(file))
  vim.cmd("2,3SvaraEdit make it four")
  assert(asked == "edit", tostring(asked))
  assert(finish.options.kind == "edit", vim.inspect(finish.options))
  assert(
    finish.instruction == "make it four\n\nSource: " .. vim.fn.fnamemodify(file, ":p") .. ":2-3",
    finish.instruction
  )

  vim.cmd("SvaraJump")
  assert(notified[#notified]:find("no edit has finished"), notified[#notified])

  -- The agent changes the file on disk, and says where.
  vim.fn.writefile({ "one", "two", "three and four" }, file)
  vim.cmd("1")
  finish.on_done({ { filename = file, lnum = 3, col = 2, text = "four" } })
  assert(notified[#notified]:find(":3 — :SvaraJump", 1, true), notified[#notified])
  -- The changed file is read again; the operator is not moved.
  local lines = vim.api.nvim_buf_get_lines(0, 0, -1, false)
  assert(lines[3] == "three and four", vim.inspect(lines))
  assert(vim.api.nvim_win_get_cursor(0)[1] == 1, "the edit moved the cursor")
  vim.cmd("SvaraJump")
  assert(vim.api.nvim_win_get_cursor(0)[1] == 3, vim.inspect(vim.api.nvim_win_get_cursor(0)))
  assert(vim.fn.getqflist({ title = 0 }).title == "Svara edit: make it four")
end

print("svara picker tests passed")
