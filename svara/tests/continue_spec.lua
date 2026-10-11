-- :SvaraContinue's picker, follow-up draft, and Ask/Edit result handling.
-- nvim --headless -u NONE -l tests/continue_spec.lua
local tests = arg[0]:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path
vim.opt.runtimepath:append(root)
vim.cmd("runtime plugin/svara.lua")
local core = require("svara.core")
local pending = require("svara.pending")
local notifications = {}
vim.notify = function(message) notifications[#notifications + 1] = message end
local file = vim.fn.tempname() .. ".lua"
vim.fn.writefile({ "first", "second", "third" }, file)
vim.cmd.edit(file)
vim.api.nvim_win_set_cursor(0, { 2, 0 })
local directory = vim.fn.fnamemodify(file, ":p:h")
local interactions = {
  { id = "edit-1", kind = "edit", prompt = "fix expiry\n\nSource: somewhere" },
  { id = "ask-1", kind = "ask", prompt = "where is auth?" },
  { id = "new-1", kind = "new", prompt = "review auth" },
}
local pick, sent, failure, complete
core.recent_interactions_for_directory = function(path)
  assert(path == directory)
  return interactions, { id = "workspace-1" }
end
vim.ui.select = function(items, options, done)
  assert(items == interactions)
  assert(options.format_item(items[1]) == "edit-1 · edit: fix expiry")
  pick = done
end
core.continue_interaction = function(interaction, message, options, on_done)
  assert(options.directory == directory)
  sent = { id = interaction.id, message = message }
  if failure then return nil, failure end
  complete = on_done
  return {}, nil, { id = interaction.id }
end
local function keys(typed)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(typed, true, false, true), "x", false)
  vim.wait(20)
end
local function choose(index, args)
  pick, sent, complete = nil, nil, nil
  vim.cmd("SvaraContinue" .. (args and (" " .. args) or ""))
  assert(pick and sent == nil)
  pick(index and interactions[index] or nil)
  vim.wait(20)
end

-- Cancel the picker without opening a draft or sending.
local original_window = vim.api.nvim_get_current_win()
choose(nil)
assert(sent == nil and vim.api.nvim_get_current_win() == original_window)

-- Failed resume preserves the edit follow-up; retry processes the changed places.
choose(1, "also check revocation")
local window = vim.api.nvim_get_current_win()
local buffer = vim.api.nvim_get_current_buf()
assert(vim.api.nvim_buf_get_lines(buffer, 0, -1, false)[1] == "also check revocation")
failure = "agent has not exited yet"
keys("<CR>")
assert(vim.api.nvim_win_is_valid(window) and vim.bo[buffer].modifiable)
assert(pending.window() == nil)
assert(notifications[#notifications]:find(failure, 1, true))
failure = nil
keys("<CR>")
assert(not vim.api.nvim_win_is_valid(window))
assert(sent.id == "edit-1")
assert(sent.message == core.prompt_from_view("also check revocation", file .. ":2:1"))
assert(pending.window())
complete({ { filename = file, lnum = 3, text = "revocation" } })
assert(pending.window() == nil)
vim.cmd("SvaraJump")
assert(vim.api.nvim_win_get_cursor(0)[1] == 3)

-- An Ask follow-up replaces and opens quickfix; its range is captured before picking.
vim.cmd("1,2SvaraContinue")
pick(interactions[2])
keys("ifind the tests<Esc><CR>")
assert(sent.id == "ask-1" and sent.message:find("Source: " .. file .. ":1-2", 1, true))
complete({ { filename = file, lnum = 1, text = "tests" } })
assert(vim.fn.getqflist({ title = 0 }).title == "Svara: find the tests")
assert(pending.window() == nil)
vim.cmd("cclose")

-- New interactions receive a message, without file-result watching.
choose(3)
keys("ikeep reviewing<Esc><CR>")
assert(sent.id == "new-1" and sent.message:find("keep reviewing", 1, true))
assert(complete == nil and pending.window() == nil)
assert(notifications[#notifications] == "Svara: message sent")

interactions = {}
pick = nil
vim.cmd("SvaraContinue")
assert(pick == nil)
assert(notifications[#notifications]:find("no recent interactions", 1, true))
vim.fn.delete(file)
print("svara continue tests passed")
