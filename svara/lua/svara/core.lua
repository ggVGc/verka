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

--- The selection a new interaction here should run under.
---
--- `vim.g.svara_selection` is the answer when it is set. Otherwise the
--- Workspace's newest Session supplies one: a Workspace being worked in has
--- already been launched under something, and continuing with it beats
--- inventing a default. A provider's *declared* defaults would be the third
--- answer, and deliberately are not one — they live in the server's Rust,
--- never appear on the wire, and a copy here would drift the first time one
--- changed.
local function selection_for(styra, workspace)
  local configured = vim.g.svara_selection
  if configured and configured ~= "" then
    return configured
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
  return sessions[1].selection
end

--- Start a new interaction, in the Workspace the directory belongs to.
---
--- The directory is the editor's working directory unless told otherwise, and
--- the Workspace is whichever one covers it — the question `:Svara` exists to
--- avoid making the operator answer. `prompt` is the first turn, sent as the
--- session comes up.
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
  })
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
