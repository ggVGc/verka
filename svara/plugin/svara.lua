if vim.g.loaded_svara then
  return
end
vim.g.loaded_svara = true

local function viewed_directory()
  local path = vim.api.nvim_buf_get_name(0)
  if path == "" then
    return nil, "the current buffer has no file"
  end
  return vim.fn.fnamemodify(path, ":p:h")
end

local function interaction_name(interaction)
  local name = require("svara").given(interaction.name)
  local activity = require("svara").given(interaction.activity) or "unknown"
  local message = require("svara").given(interaction.last_message)
  local label = name or activity
  if message and message ~= "" then
    label = label .. ": " .. message:gsub("[\r\n]+", " ")
  end
  return label
end

vim.api.nvim_create_user_command("Svara", function(command)
  local core = require("svara.core")
  local directory, directory_error = viewed_directory()
  if not directory then
    vim.notify("Svara: " .. directory_error, vim.log.levels.ERROR)
    return
  end

  if command.args == "" then
    local interactions, workspace, err = core.interactions_for_directory(directory)
    if not interactions then
      vim.notify("Svara: " .. err, vim.log.levels.ERROR)
      return
    end
    if #interactions == 0 then
      vim.notify("Svara: no live interactions in this Workspace", vim.log.levels.WARN)
      return
    end
    vim.ui.select(interactions, {
      prompt = "Select Styra interaction",
      format_item = interaction_name,
    }, function(interaction)
      if not interaction then
        return
      end
      core.select_interaction(workspace.id, interaction.id)
      vim.notify("Svara: selected " .. interaction.id, vim.log.levels.INFO)
    end)
    return
  end

  local sent, err = core.send_to_selected(core.prompt_from_view(command.args, core.viewing()), {
    directory = directory,
  })
  if not sent then
    vim.notify("Svara: " .. err, vim.log.levels.ERROR)
    return
  end
  vim.notify("Svara: message sent", vim.log.levels.INFO)
end, {
  nargs = "*",
  desc = "Select or message a Styra interaction in the Workspace over the current file",
})

vim.api.nvim_create_user_command("SvaraNew", function(command)
  local core = require("svara.core")
  local directory, directory_error = viewed_directory()
  if not directory then
    vim.notify("Svara: " .. directory_error, vim.log.levels.ERROR)
    return
  end
  local session, err = core.start(core.prompt_from_view(command.args, core.viewing()), {
    directory = directory,
  })
  if not session then
    vim.notify("Svara: " .. err, vim.log.levels.ERROR)
    return
  end
  vim.notify(
    "Svara: " .. session.id .. " started on " .. require("svara").selection_name(session.selection),
    vim.log.levels.INFO
  )
end, {
  nargs = "+",
  desc = "Start a new Styra interaction in the Workspace over the current file",
})

vim.api.nvim_create_user_command("SvaraInfo", function()
  local core = require("svara.core")
  -- The other commands refuse a buffer with no file behind it, because they
  -- would have nowhere to send to. This one is most wanted exactly when
  -- something is unclear, so it answers for Neovim's working directory instead
  -- and says that is what it did.
  local directory = viewed_directory()
  local note
  if not directory then
    directory = vim.fn.getcwd()
    note = "Neovim's working directory; this buffer has no file"
  end

  local info, err = core.info({ directory = directory })
  if not info then
    vim.notify("Svara: " .. err, vim.log.levels.ERROR)
    return
  end
  local lines = core.info_lines(info)
  if note then
    lines[1] = lines[1] .. "  (" .. note .. ")"
  end
  vim.notify(table.concat(lines, "\n"), vim.log.levels.INFO)
end, {
  desc = "Show the Workspace, model and interaction Svara would use here",
})

vim.api.nvim_create_user_command("SvaraSend", function(command)
  local session_id = command.fargs[1]
  local message = table.concat(command.fargs, " ", 2)
  local sent, err = require("svara").send_message(session_id, message)
  if not sent then
    vim.notify("Svara: " .. err, vim.log.levels.ERROR)
    return
  end
  vim.notify("Svara: message sent", vim.log.levels.INFO)
end, {
  nargs = "+",
  desc = "Send a message to an existing Styra session",
})
