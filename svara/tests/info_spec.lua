-- What `:SvaraInfo` answers, without a server or a socket.
--
-- `svara.core.info` is four requests arranged into one answer, and the answer
-- is mostly interesting when part of it cannot be given: an unreachable
-- server, a directory no Workspace covers, a Workspace with no Session to take
-- a model from. Each of those is a case here, over a host of the test's own.
--
--   nvim --headless -u NONE -l tests/info_spec.lua

local script = arg[0]
local tests = script:match("^(.*)/[^/]+$") or "."
local root = tests .. "/.."
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

local core = require("svara.core")

--- A host answering each exchange from `replies`, remembering what it was
--- asked. See `tests/api_spec.lua`, where the same shape is explained.
local function fake_host(replies)
  local host = { sent = {}, null = vim.NIL, environment = {} }
  host.encode = vim.json.encode
  host.decode = vim.json.decode
  function host.getenv(name)
    return host.environment[name]
  end
  function host.exchange(_, line, _)
    local request = vim.json.decode(line)
    host.sent[#host.sent + 1] = request.operation
    local reply = table.remove(replies, 1)
    if not reply then
      error("the test ran out of replies at " .. tostring(request.operation))
    end
    if type(reply) == "string" then
      return nil, reply
    end
    return vim.json.encode(reply)
  end
  function host.defer(_, _) end
  return host
end

local function ok(response)
  return { status = "ok", response = response }
end

local function health()
  return ok({ type = "health", data = { service = "styra-server" } })
end

local function workspace(data)
  return ok({ type = "workspace_for_path", data = data })
end

local function interactions(data)
  return ok({ type = "interactions", data = data })
end

local function sessions(data)
  return ok({ type = "stored_sessions", data = data })
end

local inner = {
  id = "1790-0",
  name = "inner",
  host_path = "/home/me/verka/styra/protocol",
  git_repository = "/home/me/verka",
  path = "/state/styra/workspaces/1790-0",
  session_count = 2,
  age = "3d ago",
  created_at_ms = 1790,
  last_accessed_at_ms = 1791,
}

local function live(id, activity)
  return {
    id = id,
    workspace_id = inner.id,
    activity = activity,
    selection = { provider = "claude", model = "claude-opus-5", effort = "high" },
    workspace = inner.host_path,
  }
end

local function ask(replies, options)
  local host = fake_host(replies)
  local info, err = core.info(vim.tbl_extend("force", {
    directory = "/home/me/verka/styra/protocol/src",
    socket = "/tmp/test.sock",
    host = host,
  }, options or {}))
  assert(info, err)
  return info, host
end

local function shown(info)
  return table.concat(core.info_lines(info), "\n")
end

-- Everything answered ----------------------------------------------------

do
  vim.g.svara_selection = nil
  local info, host = ask({
    health(),
    workspace(inner),
    interactions({ live("styra-7", "running"), live("styra-8", "stopped") }),
    sessions({
      { id = "styra-8", selection = { provider = "codex", model = "gpt-5.6-terra", effort = "medium" } },
    }),
  })

  assert(vim.deep_equal(host.sent, {
    "health",
    "workspace_for_path",
    "list_interactions",
    "list_sessions",
  }), vim.inspect(host.sent))
  assert(info.socket == "/tmp/test.sock", info.socket)
  assert(info.health.service == "styra-server")
  assert(info.workspace.id == inner.id)
  -- A stopped interaction is in the Workspace and is still not an answer to
  -- "what could I send to".
  assert(#info.interactions == 1, vim.inspect(info.interactions))
  assert(info.interactions[1].id == "styra-7")
  assert(info.selection.model == "gpt-5.6-terra", vim.inspect(info.selection))
  assert(info.selection_source == "the newest Session in the Workspace")
  assert(info.selected_interaction_id == nil)

  local lines = shown(info)
  assert(lines:find("/home/me/verka/styra/protocol/src", 1, true), lines)
  assert(lines:find("styra-server at /tmp/test.sock", 1, true), lines)
  assert(lines:find("inner — /home/me/verka/styra/protocol", 1, true), lines)
  assert(lines:find("/home/me/verka", 1, true), lines)
  assert(lines:find("codex:gpt%-5%.6%-terra/medium, from the newest Session"), lines)
  assert(lines:find("nothing yet", 1, true), lines)
  assert(lines:find("1 interaction can", 1, true), lines)
end

-- A configured model, and a choice that has since stopped ----------------

do
  vim.g.svara_selection = "claude:claude-opus-5/xhigh"
  assert(core.select_interaction(inner.id, "styra-9"))
  local info, host = ask({
    health(),
    workspace(inner),
    interactions({ live("styra-7", "pending") }),
  })

  -- `vim.g.svara_selection` is the whole answer, so no Session is asked for.
  local asked = { "health", "workspace_for_path", "list_interactions" }
  assert(vim.deep_equal(host.sent, asked), vim.inspect(host.sent))
  assert(info.selection == "claude:claude-opus-5/xhigh")
  assert(info.selection_source == "vim.g.svara_selection")
  assert(info.selected_interaction_id == "styra-9")
  assert(info.selected_interaction == nil, "the remembered interaction is no longer live")
  local lines = shown(info)
  assert(lines:find("from vim.g.svara_selection", 1, true), lines)
  assert(lines:find("styra%-9 — no longer live"), lines)

  assert(core.select_interaction(inner.id, "styra-7"))
  info = ask({ health(), workspace(inner), interactions({ live("styra-7", "pending") }) })
  assert(info.selected_interaction.id == "styra-7")
  assert(shown(info):find("styra%-7 %(pending%)"), shown(info))
  vim.g.svara_selection = nil
end

-- A Workspace with no Session to take a model from -----------------------

do
  local info = ask({
    health(),
    workspace(inner),
    interactions({}),
    sessions({}),
  })
  assert(info.workspace.id == inner.id)
  assert(info.selection == nil)
  assert(info.selection_error:find("set vim.g.svara_selection"), info.selection_error)
  local lines = shown(info)
  assert(lines:find("model", 1, true), lines)
  assert(lines:find("unknown: no Session", 1, true), lines)
  assert(lines:find("0 interactions can", 1, true), lines)
end

-- The model `:SvaraNew` offers, and the one it is told instead -----------

do
  local where = "/home/me/verka/styra/protocol/src"
  local function resolve(replies)
    local host = fake_host(replies)
    local selection, source, err =
      core.selection_for_directory(where, { socket = "/tmp/test.sock", host = host })
    return selection, source, err, host
  end

  vim.g.svara_selection = nil
  local selection, source, err = resolve({
    workspace(inner),
    sessions({
      { id = "styra-8", selection = { provider = "codex", model = "gpt-5.6-terra", effort = "medium" } },
    }),
  })
  assert(selection, err)
  assert(core.selection_said(selection) == "codex:gpt-5.6-terra/medium", vim.inspect(selection))
  assert(source == "the newest Session in the Workspace", tostring(source))

  -- Nothing to offer is a reason to ask for one, so it comes back as a
  -- sentence rather than as a guess.
  local missing
  missing, _, err = resolve({ workspace(inner), sessions({}) })
  assert(not missing)
  assert(err:find("set vim.g.svara_selection"), err)

  -- A model chosen in answer is what the next start resolves to, without a
  -- Session being asked for at all.
  local name, remember_error = core.remember_selection("claude:claude-opus-5/xhigh")
  assert(name == "claude:claude-opus-5/xhigh", tostring(remember_error))
  assert(vim.g.svara_selection == name)
  local host
  selection, source, err, host = resolve({ workspace(inner) })
  assert(selection == name, vim.inspect(selection))
  assert(source == "vim.g.svara_selection", tostring(source))
  assert(vim.deep_equal(host.sent, { "workspace_for_path" }), vim.inspect(host.sent))

  -- The models to offer instead: the server's catalog, which the command
  -- asks for rather than keeping a list of. No Workspace is involved.
  local host_of_models = fake_host({
    ok({
      type = "models",
      data = {
        {
          provider = "claude",
          model = "claude-opus-5",
          efforts = { "low", "medium", "high", "xhigh", "max" },
          default_effort = "high",
        },
        {
          provider = "claude",
          model = "claude-haiku-4-5-20251001",
          efforts = {},
          default_effort = "high",
        },
      },
    }),
  })
  local models
  models, err = core.available_models({ socket = "/tmp/test.sock", host = host_of_models })
  assert(models, err)
  assert(vim.deep_equal(host_of_models.sent, { "list_models" }), vim.inspect(host_of_models.sent))
  assert(#models == 2, vim.inspect(models))
  assert(models[1].model == "claude-opus-5")
  assert(#models[1].efforts == 5, vim.inspect(models[1]))
  -- A model that takes no effort still carries the value its Selection needs.
  assert(#models[2].efforts == 0, vim.inspect(models[2]))
  assert(models[2].default_effort == "high")

  -- An unusable answer is refused where it was typed, not at the next start.
  local bad, bad_error = core.remember_selection("claude")
  assert(not bad)
  assert(bad_error:find("reasoning effort"), bad_error)
  assert(vim.g.svara_selection == name, "a bad answer does not displace a good one")
  vim.g.svara_selection = nil
end

-- A model for one kind of command alone ------------------------------------

do
  local where = "/home/me/verka/styra/protocol/src"
  local function resolve(kind, replies)
    local host = fake_host(replies)
    local selection, source, err =
      core.selection_for_directory(where, { kind = kind, socket = "/tmp/test.sock", host = host })
    return selection, source, err, host
  end

  vim.g.svara_selection = "claude:claude-opus-5/xhigh"
  local name = core.remember_selection("claude:claude-haiku-4-5-20251001/high", "ask")
  assert(vim.g.svara_ask_selection == name, tostring(vim.g.svara_ask_selection))
  assert(vim.g.svara_selection == "claude:claude-opus-5/xhigh", "the shared model was displaced")

  -- The command's own model first, the shared one for the others.
  local selection, source = resolve("ask", { workspace(inner) })
  assert(selection == name, vim.inspect(selection))
  assert(source == "vim.g.svara_ask_selection", tostring(source))
  selection, source = resolve("edit", { workspace(inner) })
  assert(selection == "claude:claude-opus-5/xhigh", vim.inspect(selection))
  assert(source == "vim.g.svara_selection", tostring(source))

  -- And :SvaraInfo says which commands have one of their own.
  local info = ask({
    health(),
    workspace(inner),
    interactions({}),
  })
  assert(#info.command_selections == 1, vim.inspect(info.command_selections))
  local lines = shown(info)
  assert(lines:find("model (ask)", 1, true), lines)
  assert(lines:find("from vim.g.svara_ask_selection", 1, true), lines)
  assert(not lines:find("model (edit)", 1, true), lines)

  vim.g.svara_selection, vim.g.svara_ask_selection = nil, nil
end

-- A directory no Workspace covers ----------------------------------------

do
  local info = ask({ health(), workspace(vim.NIL) })
  assert(info.workspace == nil)
  assert(info.workspace_error:find("no Styra Workspace covers"), info.workspace_error)
  local lines = shown(info)
  assert(lines:find("no Styra Workspace covers", 1, true), lines)
  assert(not lines:find("model", 1, true), "nothing below the Workspace can be answered")
end

-- No server --------------------------------------------------------------

do
  local info = ask({ "could not reach the Styra server" })
  assert(info.health == nil)
  assert(info.health_error == "could not reach the Styra server")
  local lines = shown(info)
  assert(lines:find("unreachable at /tmp/test.sock", 1, true), lines)
  assert(not lines:find("workspace", 1, true), "nothing below the server can be answered")
end

-- A socket that cannot even be named -------------------------------------

do
  local host = fake_host({})
  local missing, err = core.info({ directory = "/tmp", host = host })
  assert(not missing)
  assert(err:find("XDG_RUNTIME_DIR"), err)
end

print("svara info tests passed")
