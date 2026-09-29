-- The list `:SvaraNew` offers before it starts anything.
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

local started
core.start = function(_, options)
  started = options.selection
  return { id = "styra-1", selection = require("svara.api").selection(options.selection) }
end

local file = vim.fn.tempname() .. ".lua"
vim.fn.writefile({ "local a = 1" }, file)
vim.cmd.edit(file)

--- `:SvaraNew`, answering its lists with `picks` in turn — an index, or
--- nothing to escape. Returns what each list showed.
local function run(picks)
  local shown, turn = {}, 0
  vim.ui.select = function(items, options, on_choice)
    turn = turn + 1
    local labels = {}
    for _, item in ipairs(items) do
      labels[#labels + 1] = options.format_item and options.format_item(item) or tostring(item)
    end
    shown[turn] = labels
    on_choice(picks[turn] and items[picks[turn]] or nil, picks[turn])
  end
  started, vim.g.svara_selection = nil, nil
  vim.cmd("SvaraNew what does this module trust?")
  return shown
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
  assert(vim.g.svara_selection == nil, "carrying on stores nothing")
end

-- A model from the catalog, and the rungs it accepts ----------------------

do
  local shown = run({ 2, 2 })
  assert(shown[2][1] == "low", vim.inspect(shown[2]))
  assert(shown[2][2] == "high (default)", vim.inspect(shown[2]))
  assert(started == "codex:gpt-6-astra/high", vim.inspect(started))
  assert(vim.g.svara_selection == "codex:gpt-6-astra/high", tostring(vim.g.svara_selection))
end

-- A model with no ladder is not asked about ------------------------------

do
  local shown = run({ 3 })
  assert(#shown == 1, vim.inspect(shown))
  -- It still needs an effort in its Selection: the one a launch would use.
  assert(
    vim.g.svara_selection == "claude:claude-haiku-4-5-20251001/high",
    tostring(vim.g.svara_selection)
  )
end

-- Typing one out, for an id newer than the server's tables ----------------

do
  vim.ui.input = function(_, on_input)
    on_input("claude:claude-fable-9/max")
  end
  run({ 4 })
  assert(started == "claude:claude-fable-9/max", vim.inspect(started))
  assert(vim.g.svara_selection == "claude:claude-fable-9/max")

  -- An unusable one is refused, and nothing starts.
  vim.ui.input = function(_, on_input)
    on_input("claude")
  end
  run({ 4 })
  assert(started == nil, vim.inspect(started))
  assert(notified[#notified]:find("reasoning effort"), notified[#notified])
end

-- Escaping the list ------------------------------------------------------

do
  run({})
  assert(started == nil, vim.inspect(started))
  assert(notified[#notified] == "Svara: nothing started", notified[#notified])
end

-- Nothing in use: the catalog is the whole list ---------------------------

do
  core.selection_for_directory = function()
    return nil, nil, "no Session in Workspace \"verka\" to take a model from"
  end
  local shown = run({ 1, 2 })
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

print("svara picker tests passed")
