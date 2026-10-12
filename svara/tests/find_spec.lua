-- What `:SvaraAsk` asks and puts in the quickfix list, without a server, and
-- the float that says it is still asking.
--
--   nvim --headless -u NONE -l tests/find_spec.lua

local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

local core = require("svara.core")
local pending = require("svara.pending")

--- A host answering each exchange from `replies`, with timers that run when
--- the test says. See `tests/api_spec.lua`, where the same shape is explained.
local function fake_host(replies)
  local host = { sent = {}, timers = {}, null = vim.NIL, environment = {} }
  host.encode = vim.json.encode
  host.decode = vim.json.decode
  function host.getenv(name)
    return host.environment[name]
  end
  function host.exchange(_, line, _)
    local request = vim.json.decode(line)
    host.sent[#host.sent + 1] = request
    local reply = table.remove(replies, 1)
    if not reply then
      error("the test ran out of replies at " .. tostring(request.operation))
    end
    return vim.json.encode(reply)
  end
  function host.defer(_, fn)
    host.timers[#host.timers + 1] = fn
    return function() end
  end
  function host.tick()
    local timers = host.timers
    host.timers = {}
    for _, fn in ipairs(timers) do
      fn()
    end
    return #timers
  end
  return host
end

local function ok(response)
  return { status = "ok", response = response }
end

local function sequenced(from, updates)
  local list = {}
  for index, update in ipairs(updates) do
    list[index] = { sequence = from + index, update = update }
  end
  return { updates = list, next = from + #updates }
end

-- A Workspace on disk, and an interaction running in a checkout of its own
-- inside it, so both places a relative path can be found are real.
local home = vim.fn.tempname()
local checkout = home .. "/checkout"
vim.fn.mkdir(checkout .. "/src", "p")
vim.fn.mkdir(home .. "/docs", "p")
vim.fn.writefile({ "" }, checkout .. "/src/auth.rs")
vim.fn.writefile({ "" }, home .. "/docs/auth.md")

local workspace = {
  id = "1790-0",
  name = "home",
  host_path = home,
  path = "/state/styra/workspaces/1790-0",
  session_count = 1,
  age = "now",
  created_at_ms = 1790,
  last_accessed_at_ms = 1791,
}

local selection = { provider = "claude", model = "claude-opus-5", effort = "high" }

-- What `create_session` answers: a new interaction, in a checkout of its own.
local function created(id)
  return ok({ type = "session_created", data = {
    id = id,
    workspace_id = workspace.id,
    selection = selection,
    workspace = checkout,
    journal_path = "/state/" .. id .. ".jsonl",
    driva = {},
    updates_after = 0,
  } })
end

local function find(replies, on_done, options)
  local host = fake_host(replies)
  local handle, err, session = core.find("where is auth?", vim.tbl_extend("force", {
    directory = home,
    socket = "/tmp/test.sock",
    host = host,
    selection = "claude:claude-opus-5/high",
  }, options or {}), on_done or function() end)
  return handle, err, session, host
end

-- Nothing to take a model from ---------------------------------------------

do
  vim.g.svara_selection = nil
  local handle, err, _, host = find({
    ok({ type = "workspace_for_path", data = workspace }),
    ok({ type = "stored_sessions", data = {} }),
  }, nil, { selection = false })
  assert(not handle)
  assert(err:find("vim.g.svara_selection", 1, true), err)
  for _, request in ipairs(host.sent) do
    assert(request.operation ~= "create_session", "started on no model")
  end
end

-- Answered, in an interaction of its own -------------------------------------

do
  local done
  local handle, err, session, host = find({
    ok({ type = "workspace_for_path", data = workspace }),
    created("styra-9"),
    ok({ type = "updates", data = sequenced(0, {
      { type = "event", data = { type = "turn_started" } },
    }) }),
    ok({ type = "updates", data = sequenced(1, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "answer", data = {
      contract = "files",
      value = { contract = "files", value = {
        { path = "src/auth.rs", line = 12, column = 5, description = "the check" },
        { path = "docs/auth.md", line = 66, end_line = 79, description = "the flush" },
        { path = "/abs/elsewhere.rs", line = 3 },
        { path = "gone.rs", line = 1 },
      } },
      source = "…",
    } }),
    ok({ type = "accepted" }),
  }, function(items, missed, answer, completion_error)
    done = { items = items, missed = missed, answer = answer, completion_error = completion_error }
  end)
  assert(handle, err)
  assert(session.id == "styra-9")
  -- The question is the new interaction's first turn, not a message to one
  -- already running.
  local create = host.sent[2]
  assert(create.operation == "create_session", create.operation)
  assert(create.data.workspace_id == workspace.id)
  assert(create.data.message == "where is auth?")
  assert(create.data.contract == "files", vim.inspect(create.data))
  assert(create.data.focus == true, "Styra was not asked to show the question")
  for _, request in ipairs(host.sent) do
    assert(request.operation ~= "send_message", "an existing interaction was asked")
  end
  assert(host.sent[3].data.id == "styra-9")
  assert(host.sent[3].data.after == 0)
  assert(done == nil, "answered before the turn was over")

  host.tick()
  assert(done, "the turn completed and nothing was said")
  assert(done.missed == nil, done.missed)
  assert(host.sent[#host.sent - 1].operation == "turn_answer")
  assert(host.sent[#host.sent - 1].data.id == "styra-9")
  -- Asked and answered: the interaction is done with.
  local marked = host.sent[#host.sent]
  assert(marked.operation == "set_session_completed", marked.operation)
  assert(marked.data.id == "styra-9")
  assert(marked.data.completed == "completed")
  assert(done.completion_error == nil, done.completion_error)
  local items = done.items
  assert(#items == 4)
  -- The interaction's own checkout first, then the Workspace's directory.
  assert(items[1].filename == checkout .. "/src/auth.rs", items[1].filename)
  assert(items[1].lnum == 12 and items[1].col == 5)
  assert(items[1].text == "the check")
  assert(items[2].filename == home .. "/docs/auth.md", items[2].filename)
  -- A range: its first line to jump to, its last for the quickfix list.
  assert(items[2].lnum == 66 and items[2].end_lnum == 79, vim.inspect(items[2]))
  assert(items[2].col == nil)
  assert(items[2].text == "the flush")
  assert(items[3].filename == "/abs/elsewhere.rs")
  -- Found nowhere: still said, where the agent would have meant it.
  assert(items[4].filename == checkout .. "/gone.rs", items[4].filename)

  vim.fn.setqflist({}, " ", { items = items })
  local list = vim.fn.getqflist()
  assert(#list == 4)
  assert(list[1].lnum == 12 and list[1].col == 5)
  assert(list[2].lnum == 66 and list[2].end_lnum == 79, vim.inspect(list[2]))
  assert(list[3].lnum == 3)
end

-- The model the rules give, when none is named ---------------------------------

do
  vim.g.svara_selection = "codex:gpt-5.6-terra/medium"
  local _, _, _, host = find({
    ok({ type = "workspace_for_path", data = workspace }),
    created("styra-10"),
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
  }, nil, { selection = false })
  assert(host.sent[2].data.selection.model == "gpt-5.6-terra", vim.inspect(host.sent[2].data))
  vim.g.svara_selection = nil
end

-- A reply with no locations in it --------------------------------------------

do
  local done
  local _, _, _, host = find({
    ok({ type = "workspace_for_path", data = workspace }),
    created("styra-11"),
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
    ok({ type = "updates", data = sequenced(0, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "answer", data = {
      contract = "files",
      value = vim.NIL,
      error = "the answer block names no file locations",
      source = "nothing handles auth",
    } }),
    { status = "error", error = "unknown session" },
  }, function(items, missed, answer, completion_error)
    done = { items = items, missed = missed, answer = answer, completion_error = completion_error }
  end)
  host.tick()
  -- A reply that missed its contract has still answered, so it is marked;
  -- the marking failing is said beside the answer rather than instead of it.
  assert(host.sent[#host.sent].operation == "set_session_completed")
  assert(done.completion_error == "unknown session", tostring(done.completion_error))
  assert(done.items == nil)
  assert(done.missed == "the answer block names no file locations", done.missed)
  assert(done.answer.source == "nothing handles auth")
end

-- Ended without answering: left active, to be looked at --------------------

do
  local done
  local _, _, _, host = find({
    ok({ type = "workspace_for_path", data = workspace }),
    created("styra-12"),
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
    ok({ type = "updates", data = sequenced(0, {
      { type = "event", data = { type = "error", message = "the agent fell over" } },
    }) }),
  }, function(items, missed, answer, completion_error)
    done = { items = items, missed = missed, answer = answer, completion_error = completion_error }
  end)
  host.tick()
  assert(done.missed == "the agent fell over", tostring(done.missed))
  assert(done.answer == nil)
  for _, request in ipairs(host.sent) do
    assert(request.operation ~= "set_session_completed", "a failed question was marked completed")
  end
end

-- An edit: the same turn, asked to change the files ------------------------

do
  local done
  local host = fake_host({
    ok({ type = "workspace_for_path", data = workspace }),
    created("styra-13"),
    ok({ type = "updates", data = { updates = {}, next = 0 } }),
    ok({ type = "updates", data = sequenced(0, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "answer", data = {
      contract = "files",
      value = { contract = "files", value = {
        { path = "src/auth.rs", line = 12, end_line = 14, description = "checks the expiry" },
      } },
      source = "…",
    } }),
    ok({ type = "accepted" }),
  })
  vim.g.svara_edit_selection = "codex:gpt-5.6-terra/high"
  local handle, err = core.edit("check the expiry too\n\nSource: src/auth.rs:12-14", {
    directory = home,
    socket = "/tmp/test.sock",
    host = host,
    kind = "edit",
  }, function(items)
    done = items
  end)
  vim.g.svara_edit_selection = nil
  assert(handle, err)
  local create = host.sent[2]
  assert(create.operation == "create_session", create.operation)
  -- The command's own model.
  assert(create.data.selection.model == "gpt-5.6-terra", vim.inspect(create.data.selection))
  assert(create.data.contract == "files")
  assert(create.data.focus == true, "Styra was not asked to show the edit")
  assert(not create.data.create_worktree, "the edit was made away from the open files")
  assert(vim.startswith(create.data.message, "check the expiry too\n\nSource: src/auth.rs:12-14\n\n"))
  assert(create.data.message:find(core.edit_instructions, 1, true), create.data.message)

  host.tick()
  assert(host.sent[#host.sent].operation == "set_session_completed")
  assert(#done == 1)
  assert(done[1].filename == checkout .. "/src/auth.rs", done[1].filename)
  assert(done[1].lnum == 12 and done[1].end_lnum == 14, vim.inspect(done[1]))

  local missing, missing_error = core.edit("  ", { directory = home })
  assert(not missing)
  assert(missing_error == "an edit needs an instruction", missing_error)
end

-- Continue a completed edit: resume it, watch only the new turn, complete again.

do
  local recent_host = fake_host({ ok({ type = "workspace_for_path", data = workspace }) })
  local recent = assert(core.recent_interactions_for_directory(home, { socket = "/tmp/test.sock", host = recent_host }))
  assert(recent[1].id == "styra-13" and recent[1].kind == "edit")
  assert(recent[2].id == "styra-12", "recent starts should be newest first")
  local interaction = recent[1]
  local resumed = created(interaction.id)
  resumed.response.type = "session_resumed"
  local host = fake_host({
    ok({ type = "workspace_for_path", data = workspace }),
    ok({ type = "interactions", data = {} }),
    resumed,
    -- This is the completion from the previous turn; it must not answer this one.
    ok({ type = "updates", data = sequenced(8, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "accepted" }),
    ok({ type = "updates", data = { updates = {}, next = 9 } }),
    ok({ type = "updates", data = sequenced(9, {
      { type = "event", data = { type = "turn_completed", usage = {} } },
    }) }),
    ok({ type = "answer", data = {
      contract = "files", source = "updated",
      value = { contract = "files", value = { { path = "src/auth.rs", line = 14 } } },
    } }),
    ok({ type = "accepted" }),
  })
  local done
  local handle, err, session = core.continue_interaction(interaction, "also check revocation", {
    directory = home, socket = "/tmp/test.sock", host = host,
  }, function(items) done = items end)
  assert(handle, err)
  assert(session.id == interaction.id)
  assert(host.sent[3].operation == "resume_session")
  assert(host.sent[3].data.id == interaction.id)
  assert(host.sent[5].operation == "send_message")
  assert(host.sent[5].data.message.contract == "files")
  assert(host.sent[5].data.message.text == core.edit_prompt("also check revocation"), vim.inspect(host.sent[5]))
  assert(host.sent[6].data.after == 9)
  assert(done == nil, "the old completion was mistaken for the follow-up")
  host.tick()
  assert(#done == 1 and done[1].lnum == 14)
  assert(host.sent[#host.sent].operation == "set_session_completed")
  for _, request in ipairs(host.sent) do
    assert(request.operation ~= "create_session", "the follow-up started a new conversation")
  end

  local refreshed_host = fake_host({ ok({ type = "workspace_for_path", data = workspace }) })
  local refreshed = assert(core.recent_interactions_for_directory(home, {
    socket = "/tmp/test.sock", host = refreshed_host,
  }))
  assert(refreshed[1].id == interaction.id)
  assert(refreshed[1].prompt == core.edit_prompt("also check revocation"))

  local live = fake_host({
    ok({ type = "workspace_for_path", data = workspace }),
    ok({ type = "interactions", data = { { id = "new-1", activity = "pending" } } }),
    ok({ type = "accepted" }),
  })
  local sent, send_error = core.continue_interaction({
    id = "new-1", kind = "new", session = { id = "new-1", workspace_id = workspace.id },
  }, "keep going", { directory = home, socket = "/tmp/test.sock", host = live })
  assert(sent, send_error)
  assert(#live.sent == 3 and live.sent[3].operation == "send_message")

  -- Resume failures send nothing.
  local failed = fake_host({
    ok({ type = "workspace_for_path", data = workspace }),
    ok({ type = "interactions", data = {} }),
    { status = "error", error = "agent has not exited yet" },
  })
  local missing, missed = core.continue_interaction(interaction, "retry", {
    directory = home, socket = "/tmp/test.sock", host = failed,
  }, function() error("unexpected answer") end)
  assert(not missing and missed == "agent has not exited yet")
  assert(#failed.sent == 3)

  local other_workspace = vim.tbl_extend("force", workspace, { id = "other" })
  local other = fake_host({ ok({ type = "workspace_for_path", data = other_workspace }) })
  assert(#assert(core.recent_interactions_for_directory(home, { socket = "/tmp/test.sock", host = other })) == 0)
  other = fake_host({ ok({ type = "workspace_for_path", data = other_workspace }) })
  local wrong, wrong_error = core.continue_interaction(interaction, "wrong workspace", {
    directory = home, socket = "/tmp/test.sock", host = other,
  }, function() end)
  assert(not wrong and wrong_error == "the interaction belongs to another Workspace")
end

-- Where the operator is: the cursor, or the lines selected -----------------

do
  local file = home .. "/docs/auth.md"
  vim.fn.writefile({ "a", "b", "c", "d" }, file)
  vim.cmd.edit(file)
  vim.api.nvim_win_set_cursor(0, { 2, 0 })
  assert(core.viewing() == file .. ":2:1", core.viewing())
  assert(core.viewing(0, { 2, 4 }) == file .. ":2-4", core.viewing(0, { 2, 4 }))
  assert(core.viewing(0, { 3, 3 }) == file .. ":3", core.viewing(0, { 3, 3 }))
  vim.cmd("bwipeout!")
end

-- The float --------------------------------------------------------------

do
  assert(pending.window() == nil)
  local first = pending.add("where is auth?")
  local second = pending.add("which tests cover the parser?")
  local window = assert(pending.window(), "nothing shown while asking")
  assert(not vim.api.nvim_win_get_config(window).focusable, "the float can take the focus")
  assert(vim.api.nvim_get_current_win() ~= window, "the float took the focus")
  local lines = vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(window), 0, -1, false)
  assert(#lines == 2, vim.inspect(lines))
  assert(lines[1]:find("where is auth?", 1, true), lines[1])
  assert(lines[2]:find("which tests cover the parser?", 1, true), lines[2])

  first()
  first()
  assert(vim.deep_equal(pending.labels(), { "which tests cover the parser?" }))
  assert(pending.window() == window, "the float went with a request still running")
  second()
  assert(pending.window() == nil, "the float stayed with nothing running")

  local long = pending.line({ label = string.rep("x", 200), started = 0 }, 3000, "⠋")
  assert(vim.fn.strdisplaywidth(long) <= 60, long)
  assert(long:find("… 3s$"), long)
end

-- The markers left where a request was made --------------------------------

do
  local markers = require("svara.markers")
  local file = home .. "/docs/auth.md"
  vim.fn.writefile({ "a", "b", "c", "d" }, file)
  vim.cmd.edit(file)
  local buffer = vim.api.nvim_get_current_buf()

  local function text_on(row)
    local found = vim.api.nvim_buf_get_extmarks(buffer, markers.namespace, { row, 0 }, { row, -1 }, { details = true })
    return found[1] and found[1][4].virt_text[1][1]
  end

  local scratch = vim.api.nvim_create_buf(false, true)
  assert(markers.place(scratch, 1, "nowhere", "ask") == nil, "a buffer with no file got a marker")
  markers.finish(nil, {})

  local marker = assert(markers.place(buffer, 3, "where is auth?", "ask"))
  assert(text_on(2):find("asking: where is auth?", 1, true), text_on(2))
  assert(not text_on(2):find(":SvaraAck", 1, true), "a waiting marker offers to be acknowledged")

  -- It follows its line as lines are added above it.
  vim.api.nvim_buf_set_lines(buffer, 0, 0, false, { "new" })
  assert(markers.row(marker) == 3, tostring(markers.row(marker)))

  vim.api.nvim_win_set_cursor(0, { 1, 0 })
  assert(markers.near_cursor() == marker, "no marker found in the buffer")
  assert(markers.near_cursor(true) == nil, "a waiting marker counted as finished")

  markers.finish(marker, {
    items = { { filename = file, lnum = 2, text = "the check" } },
    answer = { source = "It is checked here." },
  })
  assert(text_on(3):find("✓ svara answered: where is auth?", 1, true), text_on(3))
  assert(markers.near_cursor(true) == marker)

  local lines, places = markers.view_lines(marker)
  assert(lines[1] == "Asked: where is auth?", lines[1])
  assert(vim.tbl_contains(lines, "It is checked here."), vim.inspect(lines))
  assert(lines[#lines]:find(":2 — the check", 1, true), lines[#lines])
  assert(places[#lines].lnum == 2)

  local window = markers.view(marker)
  assert(vim.api.nvim_get_current_win() == window)
  vim.api.nvim_win_set_cursor(window, { #lines, 0 })
  vim.api.nvim_feedkeys(vim.keycode("<CR>"), "x", false)
  assert(not vim.api.nvim_win_is_valid(window), "the view stayed open")
  assert(vim.api.nvim_win_get_cursor(0)[1] == 2)

  local failed = assert(markers.place(buffer, 1, "make it four", "edit"))
  markers.finish(failed, { error = "no <styra:answer>", answer = { source = "I could not" } })
  assert(text_on(0):find("✗ svara failed: make it four", 1, true), text_on(0))
  lines = markers.view_lines(failed)
  assert(vim.tbl_contains(lines, "Failed: no <styra:answer>"), vim.inspect(lines))
  assert(vim.tbl_contains(lines, "I could not"), vim.inspect(lines))

  -- A follow-up is another turn on the same marker.
  markers.follow(marker, "and where is it refreshed?")
  assert(text_on(3):find("⧗ svara following up: and where is it refreshed? · 2 turns", 1, true), text_on(3))
  lines = markers.view_lines(marker)
  assert(lines[1] == "Asked: where is auth?", lines[1])
  assert(vim.tbl_contains(lines, "Follow-up: and where is it refreshed?"), vim.inspect(lines))
  assert(lines[#lines] == "Still following up…", lines[#lines])
  markers.finish(marker, { items = {}, answer = { source = "In the middleware." } })
  assert(text_on(3):find("✓ svara answered: and where is it refreshed? · 2 turns", 1, true), text_on(3))
  assert(vim.tbl_contains(markers.view_lines(marker), "In the middleware."))

  -- One that never started is taken back, and the marker says what it did.
  markers.follow(marker, "never sent")
  markers.unfollow(marker)
  assert(#marker.turns == 2 and marker.state == "done")

  -- `f` in the view hands the marker to whoever can follow it up.
  local followed
  marker.follow_up = function(which)
    followed = which
  end
  window = markers.view(marker)
  vim.api.nvim_feedkeys("f", "x", false)
  assert(followed == marker, "f did not follow up")
  assert(not vim.api.nvim_win_is_valid(window), "the view stayed open")

  markers.remove(marker)
  markers.remove(failed)
  assert(#markers.all() == 0)
  assert(#vim.api.nvim_buf_get_extmarks(buffer, markers.namespace, 0, -1, {}) == 0, "an extmark was left")
  vim.cmd("bwipeout!")
end

vim.fn.delete(home, "rf")
print("svara find tests passed")
