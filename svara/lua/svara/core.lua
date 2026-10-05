-- The whole-task functions the commands are made of.
--
-- Sending a message to a live session is what the `:SvaraSend` command and the
-- `bin/svara` script do, and they did it here before there was an API. They
-- still call this; it is now one line of `svara.api`, so there is one socket
-- and one copy of the wire format rather than two.
--
-- `start` joined it for `:Svara`, and is here for the same reason: it is
-- several requests arranged into one thing an operator asked for, which is a
-- thing a command does, not a thing the API should grow a special case for.

local M = {}

-- The current interaction is an editor concern, not server state: different
-- Neovim instances may quite reasonably be looking at different interactions
-- in the same Workspace.  Keeping this table here consequently makes a choice
-- last for this Neovim session and no longer.
local selected_interactions = {}
local accepting_activity = { pending = true, running = true, background = true }

--- The selection a new interaction here should run under.
---
--- `vim.g.svara_selection` is the answer when it is set. Otherwise the
--- Workspace's newest Session supplies one: a Workspace being worked in has
--- already been launched under something, and continuing with it beats
--- inventing a default. A provider's *declared* defaults would be the third
--- answer, and deliberately are not one — they live in the server's Rust,
--- never appear on the wire, and a copy here would drift the first time one
--- changed.
---
--- Where the answer came from comes back with it, because that is half of what
--- `:SvaraInfo` is asking: a model an operator did not expect is a question
--- about which of the two rules above produced it.
---@return string|table? selection
---@return string? error
---@return string? source
local function selection_for(styra, workspace)
  local configured = vim.g.svara_selection
  if configured and configured ~= "" then
    return configured, nil, "vim.g.svara_selection"
  end
  local sessions, err = styra:sessions(workspace.id)
  if not sessions then
    return nil, err
  end
  if not sessions[1] then
    local named = require("svara.api").given(workspace.name) or workspace.id
    return nil,
      string.format(
        "no Session in Workspace %q to take a model from; set vim.g.svara_selection "
          .. 'to a profile name, e.g. "claude:claude-opus-5/xhigh"',
        named
      )
  end
  return sessions[1].selection, nil, "the newest Session in the Workspace"
end

--- The selection a new interaction in `directory` would run under, for a
--- command that wants to put it to the operator before it is used.
---
--- `start` resolves the same thing silently, and for a command that is one
--- question too few: starting an interaction is the moment the answer stops
--- being cheap to change. So `:SvaraNew` asks this first, shows the answer
--- and where it came from, and only then starts.
---@param directory string
---@param options? { socket?: string, timeout?: integer, host?: table }
---@return string|table? selection
---@return string? source
---@return string? error
function M.selection_for_directory(directory, options)
  options = options or {}
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, nil, err
  end
  local workspace, workspace_error = styra:workspace_for_path(directory)
  if not workspace then
    return nil, nil, workspace_error
  end
  local selection, selection_error, source = selection_for(styra, workspace)
  if not selection then
    return nil, nil, selection_error
  end
  return selection, source
end

--- Every model a new interaction could run on, most capable first, each with
--- the reasoning-effort rungs it accepts.
---
--- What to offer an operator who did not want the one they were offered. The
--- catalog is the server's — `list_models`, the same tables the Styra TUI's
--- launcher is built from — because a list written out here would be the copy
--- of the agents' catalogs `svara.api.selection` refuses to keep, offering a
--- rung a model rejects the first time one changed. Each entry is a
--- `ModelSummary`: `provider`, `model`, `efforts` lowest first, and the
--- `default_effort` to use when nothing names one.
---@param options? { socket?: string, timeout?: integer, host?: table }
---@return table[]? models
---@return string? error
function M.available_models(options)
  local styra, err = require("svara.api").open(options or {})
  if not styra then
    return nil, err
  end
  return styra:models()
end

--- Remember a selection as the one new interactions run under.
---
--- This is `vim.g.svara_selection`, the first of the two rules above, so a
--- model chosen once holds for the rest of this Neovim session instead of
--- being asked for again at every `:SvaraNew`. It is validated before it is
--- stored, because the alternative is a profile name that reads fine and only
--- fails at the next start.
---@param value string|table
---@return string? profile_name
---@return string? error
function M.remember_selection(value)
  local api = require("svara.api")
  local picked, err = api.selection(value)
  if not picked then
    return nil, err
  end
  local name = api.selection_name(picked)
  vim.g.svara_selection = name
  return name
end

--- The interactions in one Workspace that can still be sent a message.
---
--- `list_interactions` also retains stopped interactions so clients can
--- inspect their history. A Svara choice must be something it can send to.
local function live_interactions(styra, workspace)
  local all, err = styra:interactions()
  if not all then
    return nil, err
  end
  local interactions = {}
  for _, interaction in ipairs(all) do
    if interaction.workspace_id == workspace.id and accepting_activity[interaction.activity] then
      interactions[#interactions + 1] = interaction
    end
  end
  return interactions
end

--- Start a new interaction, in the Workspace the directory belongs to.
---
--- The directory is the editor's working directory unless told otherwise, and
--- the Workspace is whichever one covers it — the question `:Svara` exists to
--- avoid making the operator answer. `prompt` is the first turn, sent as the
--- session comes up. A Styra already showing that Workspace switches to the
--- new interaction, since starting it from the editor is asking to watch it.
---@param prompt string
---@param options? { directory?: string, selection?: string|table, name?: string, contract?: string, socket?: string, timeout?: integer }
---@return table? session_info
---@return string? error
function M.start(prompt, options)
  options = options or {}
  if type(prompt) ~= "string" or prompt:match("^%s*$") then
    return nil, "a new interaction needs a prompt"
  end
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, err
  end
  local directory = options.directory or (vim.uv or vim.loop).cwd()
  local workspace, workspace_error = styra:workspace_for_path(directory)
  if not workspace then
    return nil, workspace_error
  end
  local selection, selection_error = options.selection, nil
  if not selection then
    selection, selection_error = selection_for(styra, workspace)
    if not selection then
      return nil, selection_error
    end
  end
  return styra:create_session(workspace.id, selection, {
    message = prompt,
    name = options.name,
    contract = options.contract,
    focus = true,
  })
end

--- The live interactions in the Workspace covering `directory`.
---
--- The server lists all of its live interactions.  An editor chooses from the
--- smaller list that belongs to the file it is currently showing, so a choice
--- in one Workspace cannot accidentally become the choice in another.
---@param directory string
---@param options? { socket?: string, timeout?: integer, host?: table }
---@return table? interactions
---@return table? workspace
---@return string? error
function M.interactions_for_directory(directory, options)
  options = options or {}
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, nil, err
  end
  local workspace, workspace_error = styra:workspace_for_path(directory)
  if not workspace then
    return nil, nil, workspace_error
  end
  local interactions, interactions_error = live_interactions(styra, workspace)
  if not interactions then
    return nil, nil, interactions_error
  end
  return interactions, workspace
end

--- Remember the interaction selected for one Workspace in this Neovim session.
---@param workspace_id string
---@param interaction_id string
---@return boolean? selected
---@return string? error
function M.select_interaction(workspace_id, interaction_id)
  if type(workspace_id) ~= "string" or workspace_id == "" then
    return nil, "the Workspace id must be a non-empty string"
  end
  if type(interaction_id) ~= "string" or interaction_id == "" then
    return nil, "the interaction id must be a non-empty string"
  end
  selected_interactions[workspace_id] = interaction_id
  return true
end

--- The interaction selected for a Workspace, if this Neovim session has one.
---@param workspace_id string
---@return string? interaction_id
function M.selected_interaction(workspace_id)
  return selected_interactions[workspace_id]
end

--- Send a message to the interaction selected for the Workspace at `directory`.
---@param message string
---@param options? { directory?: string, socket?: string, timeout?: integer, host?: table }
---@return boolean? sent
---@return string? error
function M.send_to_selected(message, options)
  options = options or {}
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, err
  end
  local directory = options.directory or (vim.uv or vim.loop).cwd()
  local workspace, workspace_error = styra:workspace_for_path(directory)
  if not workspace then
    return nil, workspace_error
  end
  local interaction_id = selected_interactions[workspace.id]
  if not interaction_id then
    local named = require("svara.api").given(workspace.name) or workspace.id
    return nil, string.format("no interaction selected for Workspace %q; run :Svara first", named)
  end
  return styra:send_message(interaction_id, message)
end

--- What Svara would do if a command ran in `directory`, as one table.
---
--- Every command here answers the same three questions silently — which
--- server, which Workspace, which interaction and model — and an operator only
--- finds out what they were answered when the result surprises them. This asks
--- them out loud, and is what `:SvaraInfo` shows.
---
--- Each field is filled in as far as the one above it allows: without a server
--- there is no Workspace to find, and without a Workspace no interactions and
--- no model. A question that could not be answered leaves its `*_error` beside
--- the empty field rather than failing the whole call, because "the Workspace
--- is X and the model is unknown because Y" is the answer worth showing.
---@param options? { directory?: string, socket?: string, timeout?: integer, host?: table }
---@return table? info
---@return string? error
function M.info(options)
  options = options or {}
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, err
  end
  local info = {
    directory = options.directory or (vim.uv or vim.loop).cwd(),
    socket = styra.socket,
  }

  info.health, info.health_error = styra:health()
  if not info.health then
    return info
  end

  info.workspace, info.workspace_error = styra:workspace_for_path(info.directory)
  if not info.workspace then
    return info
  end

  info.interactions, info.interactions_error = live_interactions(styra, info.workspace)
  info.selected_interaction_id = selected_interactions[info.workspace.id]
  for _, interaction in ipairs(info.interactions or {}) do
    if interaction.id == info.selected_interaction_id then
      -- Absent while the id is set means the remembered interaction has
      -- stopped since it was chosen, which is worth saying out loud.
      info.selected_interaction = interaction
    end
  end

  info.selection, info.selection_error, info.selection_source =
    selection_for(styra, info.workspace)
  return info
end

--- Label-value pairs as aligned lines.
local function rows_as_lines(rows)
  local width = 0
  for _, row in ipairs(rows) do
    width = math.max(width, #row[1])
  end
  local lines = {}
  for _, row in ipairs(rows) do
    lines[#lines + 1] = string.format("%-" .. width .. "s  %s", row[1], row[2])
  end
  return lines
end

--- Say a selection the way an operator wrote it: a profile name.
---
--- `vim.g.svara_selection` is already one, and is shown as set rather than
--- normalised: an unusable value there is exactly what this is meant to reveal.
function M.selection_said(selection)
  if type(selection) == "table" then
    return require("svara.api").selection_name(selection)
  end
  return tostring(selection)
end

local function interaction_said(interaction)
  local api = require("svara.api")
  local said = api.given(interaction.name) or interaction.id
  local activity = api.given(interaction.activity)
  if activity then
    said = string.format("%s (%s)", said, activity)
  end
  if api.given(interaction.name) then
    said = said .. " — " .. interaction.id
  end
  return said
end

--- `M.info` as lines to show, label first. The command is only the notify.
---@param info table
---@return string[] lines
function M.info_lines(info)
  local api = require("svara.api")
  local rows = { { "directory", info.directory } }

  if info.health then
    rows[#rows + 1] = { "server", string.format("%s at %s", info.health.service, info.socket) }
  else
    rows[#rows + 1] = {
      "server",
      string.format("unreachable at %s: %s", info.socket, info.health_error),
    }
    return rows_as_lines(rows)
  end

  local workspace = info.workspace
  if not workspace then
    rows[#rows + 1] = { "workspace", info.workspace_error }
    return rows_as_lines(rows)
  end
  local named = api.given(workspace.name) or vim.fn.fnamemodify(workspace.host_path, ":t")
  rows[#rows + 1] = {
    "workspace",
    string.format("%s — %s (%s)", named, workspace.host_path, workspace.id),
  }
  rows[#rows + 1] = { "git", api.given(workspace.git_repository) or "no checkout associated" }
  rows[#rows + 1] = { "sessions", string.format("%d stored", workspace.session_count) }

  if info.selection then
    local source = info.selection_source and (", from " .. info.selection_source) or ""
    rows[#rows + 1] = { "model", M.selection_said(info.selection) .. source }
  else
    rows[#rows + 1] = { "model", "unknown: " .. tostring(info.selection_error) }
  end

  if info.selected_interaction then
    rows[#rows + 1] = { "selected", interaction_said(info.selected_interaction) }
  elseif info.selected_interaction_id then
    rows[#rows + 1] = {
      "selected",
      info.selected_interaction_id .. " — no longer live; :Svara chooses again",
    }
  else
    rows[#rows + 1] = { "selected", "nothing yet; :Svara chooses" }
  end

  if info.interactions then
    local count = #info.interactions
    rows[#rows + 1] = {
      "live",
      string.format("%d interaction%s can take a message", count, count == 1 and "" or "s"),
    }
  else
    rows[#rows + 1] = { "live", "unknown: " .. tostring(info.interactions_error) }
  end

  return rows_as_lines(rows)
end

--- Where the operator is looking, as `path:line:column`, or nil. Both count
--- from 1, as compilers and `grep -n` do.
---
--- `:Svara` puts this after the prompt, because the prompt is almost
--- always about the thing on screen and saying so beats making the operator
--- type the path — but what was typed should lead. A buffer with no file behind it — a scratch buffer, the
--- start screen — is no location, and then there is nothing to say.
---@param window? integer
---@return string? location
function M.viewing(window)
  window = window or 0
  local buffer = vim.api.nvim_win_get_buf(window)
  local path = vim.api.nvim_buf_get_name(buffer)
  if path == "" then
    return nil
  end
  local cursor = vim.api.nvim_win_get_cursor(window)
  return string.format("%s:%d:%d", path, cursor[1], cursor[2] + 1)
end

--- The prompt `:Svara` sends: what was typed, then what is being viewed.
---@param prompt string
---@param location? string
---@return string
function M.prompt_from_view(prompt, location)
  if not location or type(prompt) ~= "string" or prompt:match("^%s*$") then
    return prompt
  end
  return string.format("%s\n\nSource: %s", prompt, location)
end

---Send a message to an existing, live Styra session.
---@param session_id string
---@param message string
---@param options? { socket?: string, timeout?: integer }
---@return boolean? sent
---@return string? error
function M.send_message(session_id, message, options)
  options = options or {}
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, err
  end
  return styra:send_message(session_id, message)
end

return M
