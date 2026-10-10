-- :Svara's box, driven with keys and a picker that takes the focus.
-- nvim --headless -u NONE -l tests/send_spec.lua
local tests = arg[0]:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path
vim.opt.runtimepath:append(root)
vim.cmd("runtime plugin/svara.lua")

local core = require("svara.core")
local compose = require("svara.compose")
local notifications = {}
vim.notify = function(message)
  notifications[#notifications + 1] = message
end
local file = vim.fn.tempname() .. ".lua"
vim.fn.writefile({ "first", "second" }, file)
vim.cmd.edit(file)
vim.api.nvim_win_set_cursor(0, { 2, 1 })
local directory = vim.fn.fnamemodify(file, ":p:h")
local workspace = { id = "workspace-1" }
local interactions = {
  { id = "styra-1", name = "Review", activity = "running", last_message = "first\nmessage" },
  { id = "styra-2", name = "Edit", activity = "pending" },
}
local choices, answer, sent, failure
core.interactions_for_directory = function(path)
  assert(path == directory, "use the original file's directory")
  return interactions, workspace
end
core.send_to_selected = function(message, options)
  assert(options.directory == directory)
  sent = { message = message, id = core.selected_interaction(workspace.id) }
  if failure then
    return nil, failure
  end
  return true
end
vim.ui.select = function(items, options, done)
  choices = items
  assert(options.format_item(items[1]) == "Review: first message")
  local buffer = vim.api.nvim_create_buf(false, true)
  local window = vim.api.nvim_open_win(buffer, true, {
    relative = "editor", width = 20, height = 2, row = 1, col = 1,
  })
  answer = function(index)
    vim.api.nvim_win_close(window, true)
    vim.api.nvim_buf_delete(buffer, { force = true })
    done(index and items[index] or nil)
    vim.wait(20)
  end
end
local function keys(typed)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(typed, true, false, true), "x", false)
  vim.wait(20)
end
local function title(window)
  local chunks = {}
  for _, chunk in ipairs(vim.api.nvim_win_get_config(window).title) do
    chunks[#chunks + 1] = chunk[1]
  end
  return table.concat(chunks)
end
local function open()
  sent, choices, answer = nil, nil, nil
  vim.cmd("Svara")
  return vim.api.nvim_get_current_win(), vim.api.nvim_get_current_buf()
end

-- No choice yet: open the box, then choose on send, preserving file context.
local window, buffer = open()
assert(title(window):find(compose.no_interaction, 1, true))
assert(choices == nil, "opening does not immediately open the picker")
keys("iReview this<CR>carefully<Esc><CR>")
assert(choices == interactions and sent == nil)
assert(vim.api.nvim_win_is_valid(window), "the draft stays under the picker")
answer(2)
assert(sent.id == "styra-2")
assert(sent.message == core.prompt_from_view("Review this\ncarefully", file .. ":2:2"), sent.message)
assert(not vim.api.nvim_win_is_valid(window))

-- Remember the interaction; change it from Insert mode without sending.
window, buffer = open()
assert(title(window):find(" Edit ", 1, true))
keys("ikeep this draft<C-l>")
answer(1)
assert(sent == nil and core.selected_interaction(workspace.id) == "styra-1")
assert(vim.api.nvim_get_current_win() == window)
assert(vim.api.nvim_buf_get_lines(buffer, 0, -1, false)[1] == "keep this draft")
assert(title(window):find(" Review ", 1, true))

-- Cancel a picker from Normal mode; keep the draft and selection.
keys("<C-l>")
answer(nil)
assert(vim.api.nvim_get_current_win() == window)
assert(title(window):find(" Review ", 1, true))
keys("<CR>")
assert(sent.id == "styra-1" and not vim.api.nvim_win_is_valid(window))

-- A failed send hands the draft back; retry and direct arguments still work.
failure = "interaction stopped"
window, buffer = open()
keys("iretry this<Esc><CR>")
assert(vim.api.nvim_win_is_valid(window) and vim.bo[buffer].modifiable)
assert(notifications[#notifications]:find(failure, 1, true))
failure = nil
keys("<CR>")
assert(not vim.api.nvim_win_is_valid(window))
vim.cmd("Svara direct message")
assert(sent.id == "styra-1" and sent.message:find("direct message", 1, true))

-- No live interactions, including a stale selection: keep the box open.
interactions = {}
window, buffer = open()
assert(title(window):find(compose.no_interaction, 1, true))
keys("idraft for later<Esc><CR>")
assert(sent == nil and vim.api.nvim_win_is_valid(window))
assert(notifications[#notifications]:find("no live interactions", 1, true))
keys("q")
assert(not vim.api.nvim_win_is_valid(window))
assert(notifications[#notifications] == "Svara: nothing sent")

vim.fn.delete(file)
print("svara send tests passed")
