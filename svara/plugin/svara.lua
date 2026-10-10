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

--- Put the model a new interaction would run under to the operator, and call
--- `on_chosen` with what they settled on.
---
--- The rules in `svara.core` always have an answer or a reason there is none,
--- and neither is worth being surprised by once a turn is already running. So
--- one list is offered: the model that would be used at the top of it, said
--- with where it came from, and under it every model the server says a
--- session can be launched on.
---
--- One list rather than a yes-or-no and then a list, because the answer to
--- "start on this?" is a model either way, and a question whose usual answer
--- is "yes" is a keystroke charged for nothing. Carrying on with what is
--- already in use stays the first thing under the cursor.
---
--- Anything but that first entry asks for a reasoning effort next, since the
--- catalog is models and the rungs each accepts. Typing
--- `claude:claude-opus-5/xhigh` out in full is last on the list, because a
--- catalog is not a closed set — an id newer than the server's tables is
--- still launchable. A model chosen here is remembered, so it is picked at
--- the start of a stretch of work and not at every `:SvaraNew` in it — and
--- remembered for `kind` of command alone, so the model questions are asked
--- on is not the one edits are made on unless it is chosen for both.
---
--- The questions are asked with `vim.ui`, so in whatever picker the operator
--- has configured. `on_cancel` is called when one is backed out of instead.
local function choose_selection(directory, kind, on_chosen, on_cancel)
  local core = require("svara.core")
  local typed_out = "another model…"
  local runs = { new = "new interactions", ask = "questions", edit = "edits" }

  local function settle(value)
    local name, err = core.remember_selection(value, kind)
    if not name then
      vim.notify("Svara: " .. err, vim.log.levels.ERROR)
      return
    end
    vim.notify(string.format("Svara: %s will run on %s", runs[kind], name), vim.log.levels.INFO)
    on_chosen(name)
  end

  local function ask_for_one(default)
    vim.ui.input({ prompt = "Model (provider:model/effort): ", default = default }, function(typed)
      if not typed or typed:match("^%s*$") then
        on_cancel()
        return
      end
      settle(vim.trim(typed))
    end)
  end

  --- The rungs one model accepts, with the one a launch would take by itself
  --- marked. A model that accepts none still needs a value in its Selection,
  --- and then there is nothing to ask.
  local function pick_effort(summary)
    local chosen = { provider = summary.provider, model = summary.model }
    local efforts = summary.efforts or {}
    if #efforts == 0 then
      chosen.effort = summary.default_effort
      settle(chosen)
      return
    end
    vim.ui.select(efforts, {
      prompt = string.format("Effort for %s:%s", summary.provider, summary.model),
      format_item = function(effort)
        return effort == summary.default_effort and (effort .. " (default)") or effort
      end,
    }, function(effort)
      if not effort then
        on_cancel()
        return
      end
      chosen.effort = effort
      settle(chosen)
    end)
  end

  local selection, source, err = core.selection_for_directory(directory, { kind = kind })
  local said = selection and core.selection_said(selection)
  if not selection then
    -- Nothing in use to put at the top: say why once, and let the catalog
    -- below be the whole answer.
    vim.notify("Svara: " .. err, vim.log.levels.WARN)
  end

  local models, models_error = core.available_models()
  if not models then
    -- Without the catalog there is still an answer to offer, and typing one
    -- out for the rest.
    vim.notify("Svara: " .. models_error, vim.log.levels.WARN)
    models = {}
  end

  local in_use = said and { said = said, selection = selection } or nil
  local choices = {}
  if in_use then
    choices[1] = in_use
  end
  for _, summary in ipairs(models) do
    choices[#choices + 1] = summary
  end
  if #choices == 0 then
    -- Nothing in use and no catalog: the list would be the escape hatch
    -- alone, so it is not a list.
    ask_for_one(nil)
    return
  end
  choices[#choices + 1] = typed_out

  vim.ui.select(choices, {
    prompt = "Model for " .. runs[kind],
    format_item = function(choice)
      if choice == typed_out then
        return choice
      end
      if choice == in_use then
        return string.format("%s (in use, from %s)", choice.said, source)
      end
      return string.format("%s:%s", choice.provider, choice.model)
    end,
  }, function(choice)
    if not choice then
      on_cancel()
    elseif choice == typed_out then
      ask_for_one(said)
    elseif choice == in_use then
      -- Already the answer the rules give, so there is nothing to remember.
      on_chosen(choice.selection)
    else
      pick_effort(choice)
    end
  end)
end

--- A selection as Styra's message box names it: the model, then the effort.
--- One that does not parse is shown as it was given, since saying what is set
--- is the point and the start will say what is wrong with it.
local function model_label(selection)
  local picked = require("svara.api").selection(selection)
  if not picked then
    return require("svara.core").selection_said(selection)
  end
  return picked.model .. " · " .. picked.effort
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
  -- Taken before the window opens, so the location is the file the operator
  -- was in when they asked rather than the window they are typing in.
  local location = core.viewing()
  -- The model is worked out before the window opens and named in its border,
  -- as Styra's message box names it: what Enter will start on is in sight
  -- while the prompt is written, and Ctrl+L is there to change it. Nothing to
  -- name means no `vim.g.svara_selection` and no Session to take one from;
  -- sending then asks first.
  local selection = core.selection_for_directory(directory, { kind = "new" })
  require("svara.compose").open({
    initial = command.args,
    model = selection and model_label(selection),
    choose_model = function(done)
      choose_selection(directory, "new", function(chosen)
        selection = chosen
        done(model_label(chosen))
      end, function()
        done(nil)
      end)
    end,
    on_cancel = function()
      vim.notify("Svara: nothing started", vim.log.levels.INFO)
    end,
    on_send = function(typed, create_worktree, progress)
      -- Run so the editor is not held while the server works — a new Git
      -- workspace is a worktree and a branch name made before the reply — and
      -- the window can show it working. A raise is a failure like any other,
      -- rather than a spinner that never stops.
      require("svara.nvim").run(function()
        local called, session, err = pcall(core.start, core.prompt_from_view(typed, location), {
          directory = directory,
          selection = selection,
          create_worktree = create_worktree,
        })
        if not called then
          session, err = nil, tostring(session)
        end
        if not session then
          progress.failed(err)
          vim.notify("Svara: " .. err, vim.log.levels.ERROR)
          return
        end
        progress.done()
        vim.notify(
          "Svara: "
            .. session.id
            .. " started on "
            .. require("svara").selection_name(session.selection)
            .. (create_worktree and " in a new Git workspace" or ""),
          vim.log.levels.INFO
        )
      end)
    end,
  })
end, {
  nargs = "*",
  desc = "Start a new Styra interaction in the Workspace over the current file",
})

--- A command whose request is one turn of a new interaction answered with
--- file locations: `:SvaraAsk` and `:SvaraEdit`, which differ in what the turn
--- is asked to do and what is done with the answer.
---
--- `spec.kind` names the command, for the model it runs on; `spec.run` is
--- `svara.core.find` or `svara.core.edit`; `spec.answered(request, items)`
--- takes the locations once there are some; `spec.box` is what the message
--- box says when nothing was typed after the command; and `spec.under_way`
--- and `spec.nothing` are the notes for a request started and a box closed.
local function files_command(spec, command)
  local core = require("svara.core")
  local directory, directory_error = viewed_directory()
  if not directory then
    vim.notify("Svara: " .. directory_error, vim.log.levels.ERROR)
    return
  end
  -- Taken now, so the location is where the operator was when they asked,
  -- not wherever a prompt or a picker left the cursor. A range given to the
  -- command — a visual selection, usually — is the location instead.
  local location = core.viewing(0, command.range > 0 and { command.line1, command.line2 } or nil)

  --- `progress` is the message box's, when the request was typed in one: it
  --- closes once the interaction is up, or hands the request back to be
  --- edited if it could not start.
  local function run(request, selection, progress)
    local finished = require("svara.pending").add(request)
    -- Run so the editor is not held while the interaction starts, which is
    -- the float's first stretch of spinning; a raise is a failure like any
    -- other, rather than a spinner that never stops.
    require("svara.nvim").run(function()
      local called, handle, err, session = pcall(spec.run, core.prompt_from_view(request, location), {
        directory = directory,
        selection = selection,
        kind = spec.kind,
      }, function(items, missed, _, completion_error)
        finished()
        if completion_error then
          vim.notify("Svara: could not mark it completed — " .. completion_error, vim.log.levels.WARN)
        end
        if not items then
          vim.notify("Svara: no locations — " .. tostring(missed), vim.log.levels.ERROR)
          return
        end
        spec.answered(request, items)
      end)
      if not called then
        handle, err = nil, tostring(handle)
      end
      if not handle then
        finished()
        if progress then
          progress.failed(err)
        end
        vim.notify("Svara: " .. err, vim.log.levels.ERROR)
        return
      end
      if progress then
        progress.done()
      end
      vim.notify("Svara: " .. spec.under_way .. " in " .. session.id, vim.log.levels.INFO)
    end)
  end

  -- Every request starts an interaction, so it needs a model the way
  -- `:SvaraNew` does, and with nothing to take one from it is asked for the
  -- same way rather than refused.
  local selection = core.selection_for_directory(directory, { kind = spec.kind })

  if command.args ~= "" then
    if selection then
      run(command.args, nil)
    else
      choose_selection(directory, spec.kind, function(chosen)
        run(command.args, chosen)
      end, function() end)
    end
    return
  end

  -- Nothing typed after the command: `:SvaraNew`'s box. The model is named in
  -- its border and Ctrl+L changes it, as there; there is no Ctrl+Enter, since
  -- the request is answered here rather than worked on in a Git workspace of
  -- its own.
  require("svara.compose").open(vim.tbl_extend("force", spec.box, {
    worktree = false,
    model = selection and model_label(selection),
    choose_model = function(done)
      choose_selection(directory, spec.kind, function(chosen)
        selection = chosen
        done(model_label(chosen))
      end, function()
        done(nil)
      end)
    end,
    on_cancel = function()
      vim.notify("Svara: " .. spec.nothing, vim.log.levels.INFO)
    end,
    on_send = function(typed, _, progress)
      run(typed, selection, progress)
    end,
  }))
end

vim.api.nvim_create_user_command("SvaraAsk", function(command)
  files_command({
    kind = "ask",
    run = require("svara.core").find,
    box = {
      title = " question ",
      placeholder = "Enter to ask · the places it names go to the quickfix list",
      sending = "asking…",
    },
    under_way = "asking",
    nothing = "nothing asked",
    answered = function(question, items)
      vim.fn.setqflist({}, " ", { title = "Svara: " .. question, items = items })
      vim.notify(
        string.format("Svara: %d location%s", #items, #items == 1 and "" or "s"),
        vim.log.levels.INFO
      )
      vim.cmd("botright copen")
    end,
  }, command)
end, {
  nargs = "*",
  range = true,
  desc = "Ask a new Styra interaction where something is, into the quickfix list",
})

-- The places the edit that finished last changed, and what it was asked, for
-- `:SvaraJump`.
local last_edit

--- An edit's changed places as a quickfix list of their own, entered at the
--- first. A list rather than only the first place, because an edit that
--- touched three files is three places to look at.
local function jump_to_edit(edit)
  vim.fn.setqflist({}, " ", { title = "Svara edit: " .. edit.instruction, items = edit.items })
  vim.cmd("cfirst")
end

vim.api.nvim_create_user_command("SvaraEdit", function(command)
  files_command({
    kind = "edit",
    run = require("svara.core").edit,
    box = {
      title = " edit ",
      placeholder = "Enter to edit · :SvaraJump goes to the change once it is made",
      sending = "starting…",
    },
    under_way = "editing",
    nothing = "nothing edited",
    answered = function(instruction, items)
      -- The agent changed the files on disk; buffers showing them are read
      -- again, so the edit is in sight rather than a warning at the next write.
      vim.cmd("silent! checktime")
      last_edit = { instruction = instruction, items = items }
      if #items == 0 then
        vim.notify("Svara: the edit names no places it changed", vim.log.levels.WARN)
        return
      end
      -- The edit finishes minutes after it was asked for, while the operator
      -- is somewhere else, so the editor is not moved: it is said where the
      -- change is, and `:SvaraJump` goes there.
      local first = items[1]
      local where = vim.fn.fnamemodify(first.filename, ":~:.") .. (first.lnum and (":" .. first.lnum) or "")
      local more = #items > 1 and string.format(" (+%d more)", #items - 1) or ""
      vim.notify(
        string.format("Svara: edited %s%s — :SvaraJump to go there", where, more),
        vim.log.levels.INFO
      )
    end,
  }, command)
end, {
  nargs = "*",
  range = true,
  desc = "Have a new Styra interaction make an edit at the cursor or selection",
})

vim.api.nvim_create_user_command("SvaraJump", function()
  if not last_edit or #last_edit.items == 0 then
    vim.notify("Svara: no edit has finished yet", vim.log.levels.WARN)
    return
  end
  jump_to_edit(last_edit)
end, {
  desc = "Go to the places the last finished :SvaraEdit changed, as a quickfix list",
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
