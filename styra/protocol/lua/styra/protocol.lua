-- The Styra client/server wire vocabulary, as a Lua module.
--
-- Generated from the Serde type definitions in `styra/protocol`; every
-- operation, field name, and enum spelling here is read out of the Rust that
-- defines the protocol, so this file cannot describe a protocol the server
-- does not speak. Do not edit it by hand.
--
--   cargo run -p protocol --bin styra-codegen -- lua
--
-- It carries no transport and no JSON codec, exactly as the Rust crate does
-- not: a request is a plain Lua table for your own encoder to serialise and
-- your own socket to carry, one JSON object per line.
--
--   local protocol = require("styra.protocol")
--   protocol.use_null(json.null)
--   local line = json.encode(protocol.request.send_message({
--     id = session, message = { text = "hello" },
--   }))
--   local answer, err = protocol.expect(json.decode(reply), protocol.Response.ANSWER)
--
local M = {}


-- Typed answers
-- -------------

--- The delimiters a contract's answer block sits in.
M.ANSWER_OPEN = "<styra:answer>"
M.ANSWER_CLOSE = "</styra:answer>"

-- Wire types
-- ----------

--- Every type that crosses the socket, described well enough to check a
--- value against it. See `M.validate`.
M.types = {}

--- One JSON request sent as a single line over the Unix socket.
M.types.Request = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "operation", content = "data" },
  deny_unknown_fields = true,
  variants = {
    { name = "integrate_session", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "message", required = true, type = { kind = "string" } },
      },
    } },
    { name = "generate_integration_message", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "generate_edit_report", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "base", required = true, type = { kind = "string" } },
        { name = "files", required = true, type = { kind = "list", item = { kind = "ref", name = "EditReportFile" } } },
      },
    } },
    { name = "edit_report", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "session_integration_result", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "rebase_session", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "retarget_session", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "target", required = true, type = { kind = "string" } },
      },
    } },
    { name = "session_branch_history", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "restore_session_branch", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "branch", required = true, type = { kind = "string" } },
      },
    } },
    { name = "session_rebase_status", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "health", payload = { kind = "unit" } },
    { name = "create_workspace", payload = { kind = "newtype", type = { kind = "ref", name = "CreateWorkspace" } } },
    { name = "list_workspaces", payload = { kind = "unit" } },
    { name = "list_deleted_workspaces", payload = { kind = "unit" } },
    { name = "delete_workspace", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "restore_workspace", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "workspace", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "workspace_for_path", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "path", required = true, type = { kind = "string", path = true } },
      },
    } },
    { name = "rename_workspace", payload = { kind = "newtype", type = { kind = "ref", name = "RenameWorkspace" } } },
    { name = "set_workspace_git_repository", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
        { name = "git_repository", required = true, type = { kind = "optional", inner = { kind = "string", path = true } } },
      },
    } },
    { name = "set_workspace_host_path", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
        { name = "host_path", required = true, type = { kind = "string", path = true } },
      },
    } },
    { name = "workspace_launch", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "create_session", payload = { kind = "newtype", type = { kind = "ref", name = "CreateSession" } } },
    { name = "plan_session", payload = { kind = "newtype", type = { kind = "ref", name = "PlanSession" } } },
    { name = "list_templates", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "list_models", payload = { kind = "unit" } },
    { name = "remember_model", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
      },
    } },
    { name = "resume_session", payload = { kind = "newtype", type = { kind = "ref", name = "ResumeSession" } } },
    { name = "create_session_worktree", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "clean_worktrees", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "list_worktrees", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "convert_session_provider", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "branch_session", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "at_ms", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
        { name = "history", required = false, type = { kind = "ref", name = "BranchHistory" } },
        { name = "provider", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Provider" } } },
        { name = "selection", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Selection" } } },
      },
    } },
    { name = "rename_session", payload = { kind = "newtype", type = { kind = "ref", name = "RenameSession" } } },
    { name = "set_session_tags", payload = { kind = "newtype", type = { kind = "ref", name = "SetSessionTags" } } },
    { name = "list_tags", payload = { kind = "unit" } },
    { name = "session_composer", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "set_session_composer", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "composer", required = true, type = { kind = "ref", name = "ComposerState" } },
      },
    } },
    { name = "transcribe_audio", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "path", required = true, type = { kind = "string", path = true } },
      },
    } },
    { name = "audio_recording_started", payload = { kind = "unit" } },
    { name = "audio_recording_stopped", payload = { kind = "unit" } },
    { name = "audio_transcription_error", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "error", required = true, type = { kind = "string" } },
      },
    } },
    { name = "change_workspace_launch", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
        { name = "change", required = true, type = { kind = "ref", name = "WorkspaceLaunchChange" } },
      },
    } },
    { name = "send_message", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "message", required = true, type = { kind = "ref", name = "SendMessage" } },
      },
    } },
    { name = "set_session_selection", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
      },
    } },
    { name = "set_interaction_working_directory", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "directory", required = true, type = { kind = "string", path = true } },
      },
    } },
    { name = "set_interaction_auto_retry", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "enabled", required = true, type = { kind = "boolean" } },
      },
    } },
    { name = "set_interaction_auto_commit", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "enabled", required = true, type = { kind = "boolean" } },
      },
    } },
    { name = "queue_message", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "message", required = true, type = { kind = "ref", name = "SendMessage" } },
      },
    } },
    { name = "send_queued_message", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "clear_queued_messages", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "interrupt_interaction", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "stop_interaction", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "set_session_completed", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "completed", required = true, type = { kind = "ref", name = "CompletionState" } },
      },
    } },
    { name = "close_interaction", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "load_interaction", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "updates", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "after", required = true, type = { kind = "number", integer = true } },
        { name = "raw", required = false, type = { kind = "boolean" } },
      },
    } },
    { name = "list_interactions", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "ref", name = "InteractionQuery" } } } },
    { name = "list_sessions", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "stored_session", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "raw", required = false, type = { kind = "boolean" } },
      },
    } },
    { name = "provider_raw", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "shell", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "start_project_action", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "action", required = true, type = { kind = "ref", name = "ProjectAction" } },
        { name = "command", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "turn_answer", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
      },
    } },
    { name = "quota_log", payload = { kind = "unit" } },
    { name = "interaction_actions", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "interaction_processes", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "interaction_driva", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "diff_base", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "snapshot_turn_diff", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "turn", required = true, type = { kind = "number", integer = true } },
      },
    } },
    { name = "shutdown", payload = { kind = "unit" } },
  },
}

--- Versioned request envelope. Flattening keeps `operation` at the top level.
--- Successful response payload. The variant must match the request operation.
M.types.Response = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "type", content = "data" },
  variants = {
    { name = "rebase_started", payload = { kind = "unit" } },
    { name = "session_branch_history", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "SavedBranch" } } } },
    { name = "session_rebase_status", payload = { kind = "newtype", type = { kind = "ref", name = "RebaseStatus" } } },
    { name = "integration_started", payload = { kind = "unit" } },
    { name = "integration_message", payload = { kind = "newtype", type = { kind = "string" } } },
    { name = "edit_report", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "ref", name = "EditReport" } } } },
    { name = "session_integration_result", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "ref", name = "IntegrationResult" } } } },
    { name = "health", payload = { kind = "newtype", type = { kind = "ref", name = "Health" } } },
    { name = "workspace_created", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspaces", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "WorkspaceSummary" } } } },
    { name = "workspace", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_for_path", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "ref", name = "WorkspaceSummary" } } } },
    { name = "workspace_renamed", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_git_repository_updated", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_host_path_updated", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_launch", payload = { kind = "newtype", type = { kind = "ref", name = "LaunchPolicy" } } },
    { name = "session_created", payload = { kind = "newtype", type = { kind = "ref", name = "SessionInfo" } } },
    { name = "session_plan", payload = { kind = "newtype", type = { kind = "ref", name = "DrivaOptions" } } },
    { name = "templates", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "TemplateSummary" } } } },
    { name = "models", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "ModelSummary" } } } },
    { name = "model_remembered", payload = { kind = "unit" } },
    { name = "session_resumed", payload = { kind = "newtype", type = { kind = "ref", name = "SessionInfo" } } },
    { name = "session_worktree_created", payload = { kind = "unit" } },
    { name = "worktrees_cleaned", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "CleanedWorktree" } } } },
    { name = "worktrees", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "WorktreeSummary" } } } },
    { name = "session_converted", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
    { name = "session_branched", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
    { name = "session_renamed", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
    { name = "session_tags_updated", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
    { name = "tags", payload = { kind = "newtype", type = { kind = "list", item = { kind = "string" } } } },
    { name = "session_composer", payload = { kind = "newtype", type = { kind = "ref", name = "ComposerState" } } },
    { name = "session_composer_stored", payload = { kind = "unit" } },
    { name = "workspace_launch_updated", payload = { kind = "newtype", type = { kind = "ref", name = "LaunchPolicy" } } },
    { name = "accepted", payload = { kind = "unit" } },
    { name = "queued", payload = { kind = "newtype", type = { kind = "number", integer = true } } },
    { name = "sent_queued_message", payload = { kind = "tuple", items = { { kind = "optional", inner = { kind = "ref", name = "QueuedMessage" } }, { kind = "list", item = { kind = "ref", name = "QueuedMessage" } } } } },
    { name = "queued_messages", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "QueuedMessage" } } } },
    { name = "interaction_loaded", payload = { kind = "newtype", type = { kind = "ref", name = "LoadedInteraction" } } },
    { name = "updates", payload = { kind = "newtype", type = { kind = "ref", name = "Updates" } } },
    { name = "interactions", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "InteractionSummary" } } } },
    { name = "stored_sessions", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "SessionSummary" } } } },
    { name = "stored_session", payload = { kind = "newtype", type = { kind = "ref", name = "StoredSession" } } },
    { name = "provider_raw", payload = { kind = "newtype", type = { kind = "ref", name = "ProviderRaw" } } },
    { name = "audio_transcript", payload = { kind = "newtype", type = { kind = "string" } } },
    { name = "shell", payload = { kind = "newtype", type = { kind = "ref", name = "ShellInfo" } } },
    { name = "project_action_started", payload = { kind = "newtype", type = { kind = "ref", name = "ProjectActionSession" } } },
    { name = "answer", payload = { kind = "newtype", type = { kind = "ref", name = "Answer" } } },
    { name = "quota_log", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "QuotaEvent" } } } },
    { name = "interaction_actions", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "ActionRecord" } } } },
    { name = "interaction_processes", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "list", item = { kind = "ref", name = "SandboxProcess" } } } } },
    { name = "interaction_driva", payload = { kind = "newtype", type = { kind = "ref", name = "DrivaOptions" } } },
    { name = "snapshot_turn_diff", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "ref", name = "TurnDiff" } } } },
    { name = "diff_base", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "ref", name = "DiffBase" } } } },
  },
}

--- Response envelope returned for every syntactically valid connection.
M.types.WireResponse = {
  kind = "enum",
  tagging = { style = "internal", tag = "status" },
  variants = {
    { name = "ok", payload = {
      kind = "struct",
      fields = {
        { name = "response", required = true, type = { kind = "ref", name = "Response" } },
      },
    } },
    { name = "error", payload = {
      kind = "struct",
      fields = {
        { name = "error", required = true, type = { kind = "string" } },
      },
    } },
  },
}

--- One file's edits, put to an agent to describe: see
--- `crate::protocol::Request::GenerateEditReport`.
M.types.EditReportFile = {
  kind = "struct",
  fields = {
    { name = "path", required = true, type = { kind = "string" } },
    { name = "patch", required = true, type = { kind = "string" } },
    { name = "fingerprint", required = true, type = { kind = "string" } },
  },
}

M.types.CreateWorkspace = {
  kind = "struct",
  fields = {
    { name = "host_path", required = true, type = { kind = "string", path = true } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "git_repository", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
  },
}

--- Change a Workspace's optional operator-facing name.
M.types.RenameWorkspace = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = true, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

M.types.CreateSession = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "launch", required = false, type = { kind = "ref", name = "LaunchPolicy" } },
    { name = "create_worktree", required = false, type = { kind = "boolean" } },
    { name = "checkout_from", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "message", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
    { name = "focus", required = false, type = { kind = "boolean" } },
  },
}

--- Ask what a new session in this Workspace *would* be launched under, without
--- creating one. Carries exactly the launch inputs of `CreateSession` that
--- shape the sandbox, so the answer is the policy that session would get.
M.types.PlanSession = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "launch", required = false, type = { kind = "ref", name = "LaunchPolicy" } },
    { name = "create_worktree", required = false, type = { kind = "boolean" } },
    { name = "checkout_from", required = false, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- What an operator picked to launch: an agent, a model, and a reasoning effort.
---
--- All three are always present. A selection never leaves the model or effort to
--- whatever the agent happens to be configured for, because that configuration is
--- invisible to Genta and to anything reading a journal afterwards — a session
--- recorded as plain `codex` says nothing about what actually ran. A profile name
--- that omits either therefore takes this provider's declared default
--- (`Provider::default_model`, `Provider::default_effort`) rather than
--- standing for "unset".
---
--- A selection round-trips through one string, `Selection::name`, of the form
--- `provider:model/effort` — `codex:gpt-5.6-terra/medium`,
--- `claude:claude-opus-5/xhigh`. That string is the profile name, so it is also
--- what a journal records and a status line shows: a stored session states which
--- model and effort ran. The host chooses execution mode separately when
--- resolving the selection. Parsing accepts
--- the shorter `provider[:model][/effort]` forms and fills in the defaults, so
--- `--profile claude` still works and names itself fully afterwards.
M.types.Selection = {
  kind = "struct",
  fields = {
    { name = "provider", required = true, type = { kind = "ref", name = "Provider" } },
    { name = "model", required = true, type = { kind = "string" } },
    { name = "effort", required = true, type = { kind = "ref", name = "Effort" } },
  },
}

M.types.ResumeSession = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "launch", required = false, type = { kind = "ref", name = "LaunchPolicy" } },
    { name = "selection", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Selection" } } },
  },
}

--- The source history used to seed a branch.
M.types.BranchHistory = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "through_selected", payload = { kind = "unit" } },
    { name = "selected_only", payload = { kind = "unit" } },
  },
}

--- Which coding agent a session launches. Model, reasoning effort, and execution
--- mode are chosen separately; a provider is only the agent itself.
M.types.Provider = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "codex", payload = { kind = "unit" } },
    { name = "claude", payload = { kind = "unit" } },
  },
}

M.types.RenameSession = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = true, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- Replace a Session's operator-assigned tags.
M.types.SetSessionTags = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "tags", required = true, type = { kind = "list", item = { kind = "string" } } },
  },
}

--- What an operator has half-written to a Session, and what they have already
--- sent it: the message box, kept by the server so it is there again from any
--- client, after the client is closed, and after the interaction is resumed.
---
--- The server stores it as given and keeps the newest
--- `COMPOSER_HISTORY_LIMIT` history entries; it does not interpret it.
M.types.ComposerState = {
  kind = "struct",
  fields = {
    { name = "parts", required = false, type = { kind = "list", item = { kind = "string" } } },
    { name = "focused", required = false, type = { kind = "number", integer = true } },
    { name = "history", required = false, type = { kind = "list", item = { kind = "string" } } },
  },
}

--- One server-owned edit to a Workspace's standing launch policy.
---
--- Clients send intent instead of replacing a locally cached copy. This lets
--- the server apply the edit to the latest stored policy and return the
--- authoritative result, avoiding lost updates between multiple Styra UIs.
M.types.WorkspaceLaunchChange = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "change", content = "value" },
  variants = {
    { name = "set_network", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "boolean" } } } },
    { name = "set_writable_workspace", payload = { kind = "newtype", type = { kind = "optional", inner = { kind = "boolean" } } } },
    { name = "set_templates", payload = { kind = "newtype", type = { kind = "list", item = { kind = "string" } } } },
    { name = "add_mounts", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "LaunchMount" } } } },
    { name = "remove_mount", payload = { kind = "newtype", type = { kind = "ref", name = "LaunchMount" } } },
    { name = "replace", payload = { kind = "newtype", type = { kind = "ref", name = "LaunchPolicy" } } },
    { name = "set_command", payload = {
      kind = "struct",
      fields = {
        { name = "action", required = true, type = { kind = "ref", name = "ProjectAction" } },
        { name = "command", required = true, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
  },
}

M.types.SendMessage = {
  kind = "struct",
  fields = {
    { name = "text", required = true, type = { kind = "string" } },
    { name = "selection", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Selection" } } },
    { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
  },
}

--- Whether the operator is finished with a Session, how, and whether that is
--- final.
---
--- `Self::Abandoned` is `Self::Completed` in everything but what it says:
--- the operator is done with the Session because they gave up on the work,
--- not because it got done. Both are reversible in the same ways.
---
--- `Self::Sealed` means the same as `Self::Completed` wherever completion
--- is read — hidden from the default listing, its interaction stopped — but
--- it is where the Session stops: no client action turns it back into
--- `Self::Active`, and the Session can never be resumed again. It is the
--- operator's final word, not a checkbox. The history stays readable, and
--- branching still takes a copy that can run; the sealed Session itself does
--- not run again.
M.types.CompletionState = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "active", payload = { kind = "unit" } },
    { name = "completed", payload = { kind = "unit" } },
    { name = "abandoned", payload = { kind = "unit" } },
    { name = "sealed", payload = { kind = "unit" } },
  },
}

--- Which interactions a `crate::protocol::Request::ListInteractions` answers
--- with. Every field narrows the listing, so the default — and a request that
--- carries no query at all — lists every row, as the request did before it
--- took one.
---
--- The roster keeps every interaction a previous server run left open until
--- the operator closes it, finished ones included, so an unnarrowed listing
--- grows with everything ever started. A client polling for the rows it shows
--- asks only for those.
M.types.InteractionQuery = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "id", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "workspace_id", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "skip_done", required = false, type = { kind = "boolean" } },
    { name = "skip_stopped", required = false, type = { kind = "boolean" } },
    { name = "keep", required = false, type = { kind = "list", item = { kind = "string" } } },
  },
}

--- A project command an operator starts from a client against a live
--- interaction, the way an IDE's run and test buttons work: it runs in the
--- interaction's own sandbox, in a terminal of its own, beside the agent.
---
--- What each one runs is a shell command resolved in layers, most specific
--- first: the interaction's `LaunchPolicy`, the Workspace's, and finally the
--- default of the Driva templates the interaction was launched with (the
--- `rust` template's is `cargo run` and `cargo test`).
M.types.ProjectAction = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "run", payload = { kind = "unit" } },
    { name = "test", payload = { kind = "unit" } },
  },
}

--- The shape a client asks a turn's answer to come back in.
---
--- A contract is applied at both ends of one turn: it frames the message sent
--- to the agent with instructions describing the shape, and it parses the
--- agent's reply back into `AnswerValue`. Framing server-side is what keeps
--- clients honest — every caller asks for a shape the same way, so the parser
--- only has to understand one phrasing. See `crate::contract`.
M.types.Contract = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "text", payload = { kind = "unit" } },
    { name = "lines", payload = { kind = "unit" } },
    { name = "files", payload = { kind = "unit" } },
    { name = "json", payload = { kind = "unit" } },
  },
}

--- A previous interaction branch, retained when its worktree changes branches.
M.types.SavedBranch = {
  kind = "struct",
  fields = {
    { name = "branch", required = true, type = { kind = "string" } },
    { name = "branched_from", required = true, type = { kind = "optional", inner = { kind = "ref", name = "BranchPoint" } } },
  },
}

--- Progress and final outcome of a standalone interaction rebase.
M.types.RebaseStatus = {
  kind = "struct",
  fields = {
    { name = "finished", required = true, type = { kind = "boolean" } },
    { name = "succeeded", required = true, type = { kind = "boolean" } },
    { name = "message", required = true, type = { kind = "string" } },
    { name = "source_commit", required = true, type = { kind = "string" } },
    { name = "output", required = false, type = { kind = "string" } },
  },
}

--- What each edited file of a Session's checkout was changed for: a short
--- description per file, written by an agent from the file's patch, the
--- Session's task, and its branch's commit messages.
M.types.EditReport = {
  kind = "struct",
  fields = {
    { name = "generated_at_ms", required = true, type = { kind = "number", integer = true } },
    { name = "base", required = true, type = { kind = "string" } },
    { name = "files", required = true, type = { kind = "list", item = { kind = "ref", name = "FileEditReport" } } },
    { name = "requested", required = true, type = { kind = "number", integer = true } },
  },
}

M.types.IntegrationResult = {
  kind = "struct",
  fields = {
    { name = "completed", required = true, type = { kind = "boolean" } },
    { name = "message", required = true, type = { kind = "string" } },
  },
}

M.types.Health = {
  kind = "struct",
  fields = {
    { name = "service", required = true, type = { kind = "string" } },
  },
}

--- A durable Styra Workspace, which groups provider Sessions that operate on
--- the same host directory.
M.types.WorkspaceSummary = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = true, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "host_path", required = true, type = { kind = "string", path = true } },
    { name = "git_repository", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
    { name = "path", required = true, type = { kind = "string", path = true } },
    { name = "session_count", required = true, type = { kind = "number", integer = true } },
    { name = "age", required = true, type = { kind = "string" } },
    { name = "created_at_ms", required = true, type = { kind = "number", integer = true } },
    { name = "last_accessed_at_ms", required = false, type = { kind = "number", integer = true } },
    { name = "launch", required = false, type = { kind = "ref", name = "LaunchPolicy" } },
  },
}

--- The sandbox policy inputs a launch asks for beyond the agent selection.
---
--- The same type serves two roles, and the difference is only where it is
--- stored. A Workspace holds one as its *standing* policy — what every launch
--- there starts from (`WorkspaceSummary::launch`). A launch request carries
--- one as its own *overlay* — what this interaction adds to, or says instead
--- of, the Workspace's. `LaunchPolicy::merge` is the only place the two are
--- combined, and the server merges them for `create_session`, `plan_session`
--- and `resume_session` alike, so a plan cannot disagree with the launch it
--- describes.
---
--- Nothing here is resolved: `templates` are names and `mounts` are requests.
--- The server resolves both against the Workspace's `driva.toml` and the host
--- filesystem after merging.
M.types.LaunchPolicy = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "network", required = false, type = { kind = "optional", inner = { kind = "boolean" } } },
    { name = "writable_workspace", required = false, type = { kind = "optional", inner = { kind = "boolean" } } },
    { name = "templates", required = false, type = { kind = "list", item = { kind = "string" } } },
    { name = "mounts", required = false, type = { kind = "list", item = { kind = "ref", name = "LaunchMount" } } },
    { name = "ignore_workspace", required = false, type = { kind = "boolean" } },
    { name = "run", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "test", required = false, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

M.types.SessionInfo = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "workspace", required = true, type = { kind = "string", path = true } },
    { name = "journal_path", required = true, type = { kind = "string", path = true } },
    { name = "driva", required = true, type = { kind = "ref", name = "DrivaOptions" } },
    { name = "updates_after", required = false, type = { kind = "number", integer = true } },
    { name = "queued", required = false, type = { kind = "list", item = { kind = "ref", name = "QueuedMessage" } } },
  },
}

--- A human-facing summary of the Driva policy an interaction was launched with:
--- the isolation backend, the command it runs, and the mount/network policy
--- enforced around it. Captured once at spawn time from the same
--- `ExecutionRequest` Driva itself executes (see `DrivaOptions::capture` in
--- `crate::interaction`), so it can never drift from what is actually running.
M.types.DrivaOptions = {
  kind = "struct",
  fields = {
    { name = "isolation_backend", required = true, type = { kind = "string" } },
    { name = "command", required = true, type = { kind = "list", item = { kind = "string" } } },
    { name = "working_directory", required = true, type = { kind = "string", path = true } },
    { name = "network", required = true, type = { kind = "boolean" } },
    { name = "mounts", required = true, type = { kind = "list", item = { kind = "ref", name = "AttributedMount" } } },
    { name = "base", required = false, type = { kind = "list", item = { kind = "ref", name = "BaseCapability" } } },
    { name = "floor", required = false, type = { kind = "list", item = { kind = "ref", name = "FloorEntry" } } },
    { name = "environment", required = false, type = { kind = "list", item = { kind = "ref", name = "AttributedVariable" } } },
    { name = "interactive", required = false, type = { kind = "boolean" } },
    { name = "new_session", required = false, type = { kind = "boolean" } },
    { name = "writable_mounts", required = false, type = { kind = "ref", name = "WritableMountMode" } },
    { name = "template_run", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "template_test", required = false, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- A Driva execution template the server can offer, named and described, so a
--- client can present the real set rather than asking the operator to recall
--- template names.
M.types.TemplateSummary = {
  kind = "struct",
  fields = {
    { name = "name", required = true, type = { kind = "string" } },
    { name = "description", required = false, type = { kind = "string" } },
  },
}

--- A model a client can offer in a picker, with the reasoning-effort rungs it
--- actually accepts.
---
--- The catalog is `crate::agent`'s: which providers Styra launches
--- interactively, which of their models are worth offering, and the per-model
--- effort ladder. Those tables are read from the agents' own catalogs and move
--- when the agents do, so a client that re-typed them would offer a rung a
--- model rejects the first time one changed. Putting them on the wire is what
--- lets a client outside this crate — an editor plugin, a status line — build
--- the same picker the TUI's launcher builds without keeping a second copy.
---
--- Advisory rather than closed: a `Selection` still carries a free-form
--- model id, and the agent is the authority that rejects one it does not know.
M.types.ModelSummary = {
  kind = "struct",
  fields = {
    { name = "provider", required = true, type = { kind = "ref", name = "Provider" } },
    { name = "model", required = true, type = { kind = "string" } },
    { name = "efforts", required = true, type = { kind = "list", item = { kind = "ref", name = "Effort" } } },
    { name = "default_effort", required = true, type = { kind = "ref", name = "Effort" } },
  },
}

--- What a worktree cleanup pass did about one Session's checkout — see
--- `crate::protocol::Request::CleanWorktrees`.
---
--- Every Session the pass considered is reported, including the ones it left
--- alone: an operator running a cleanup is asking what happened to the work
--- their agents left behind, and "this one still has uncommitted changes" is
--- the most important answer it can give them.
M.types.CleanedWorktree = {
  kind = "struct",
  fields = {
    { name = "session_id", required = true, type = { kind = "string" } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "branch", required = true, type = { kind = "string" } },
    { name = "worktree", required = true, type = { kind = "string", path = true } },
    { name = "outcome", required = true, type = { kind = "ref", name = "WorktreeCleanup" } },
  },
}

--- One linked worktree Styra knows of, and the Sessions that work in it — see
--- `crate::protocol::Request::ListWorktrees`.
---
--- A worktree is listed once however many Sessions record it, because Sessions
--- can be launched from one another's checkout: what the operator is asking
--- is which conversations touched these files, and the answer is a list.
M.types.WorktreeSummary = {
  kind = "struct",
  fields = {
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "worktree", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
    { name = "branch", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "exists", required = true, type = { kind = "boolean" } },
    { name = "sessions", required = true, type = { kind = "list", item = { kind = "ref", name = "WorktreeSession" } } },
  },
}

--- A stored session, enough to display and select it from a list — see
--- `crate::journal::list_sessions`.
M.types.SessionSummary = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "first_prompt", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "tags", required = false, type = { kind = "list", item = { kind = "string" } } },
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "path", required = true, type = { kind = "string", path = true } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "age", required = true, type = { kind = "string" } },
    { name = "created_at_ms", required = true, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "last_event_at_ms", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "last_event_age", required = false, type = { kind = "string" } },
    { name = "origin", required = false, type = { kind = "optional", inner = { kind = "ref", name = "SessionOrigin" } } },
    { name = "branched_from", required = false, type = { kind = "optional", inner = { kind = "ref", name = "BranchPoint" } } },
    { name = "completed", required = false, type = { kind = "ref", name = "CompletionState" } },
    { name = "integration", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Integration" } } },
  },
}

--- An operator message persisted but not yet sent.
---
--- Carries the contract it was queued under, so a message the operator asked
--- for a shape while the agent was busy still asks for it when the queue
--- drains. Without that the shape would be dropped at exactly the moment the
--- operator could least do anything about it.
M.types.QueuedMessage = {
  kind = "struct",
  fields = {
    { name = "text", required = true, type = { kind = "string" } },
    { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
  },
}

--- Everything a client needs to make one live interaction current, returned
--- by one ordinary blocking request.
M.types.LoadedInteraction = {
  kind = "struct",
  fields = {
    { name = "summary", required = true, type = { kind = "ref", name = "InteractionSummary" } },
    { name = "updates", required = true, type = { kind = "ref", name = "Updates" } },
    { name = "queued", required = true, type = { kind = "list", item = { kind = "ref", name = "QueuedMessage" } } },
  },
}

M.types.Updates = {
  kind = "struct",
  fields = {
    { name = "updates", required = true, type = { kind = "list", item = { kind = "ref", name = "SequencedUpdate" } } },
    { name = "next", required = true, type = { kind = "number", integer = true } },
  },
}

--- An interaction the server is currently running (this process's live sessions),
--- enough to list it and to reattach a client to it. Distinct from
--- `SessionSummary`, which describes a session persisted in the store
--- whether or not it is still live.
M.types.InteractionSummary = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "tags", required = false, type = { kind = "list", item = { kind = "string" } } },
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "workspace", required = true, type = { kind = "string", path = true } },
    { name = "activity", required = false, type = { kind = "ref", name = "InteractionActivity" } },
    { name = "activity_reason", required = false, type = { kind = "optional", inner = { kind = "ref", name = "InteractionActivityReason" } } },
    { name = "activity_since_ms", required = false, type = { kind = "number", integer = true } },
    { name = "idle_unseen", required = false, type = { kind = "boolean" } },
    { name = "interacted_at_ms", required = false, type = { kind = "number", integer = true } },
    { name = "uncommitted_changes", required = false, type = { kind = "boolean" } },
    { name = "checkout", required = false, type = { kind = "optional", inner = { kind = "ref", name = "CheckoutState" } } },
    { name = "expected_branch", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "branched_from", required = false, type = { kind = "optional", inner = { kind = "ref", name = "BranchPoint" } } },
    { name = "last_message", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "recent_messages", required = false, type = { kind = "list", item = { kind = "ref", name = "RecentMessage" } } },
    { name = "auto_retry", required = false, type = { kind = "boolean" } },
    { name = "auto_commit", required = false, type = { kind = "boolean" } },
    { name = "events", required = false, type = { kind = "number", integer = true } },
    { name = "completed", required = false, type = { kind = "ref", name = "CompletionState" } },
    { name = "integration", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Integration" } } },
    { name = "focus_requested", required = false, type = { kind = "boolean" } },
    { name = "origin", required = false, type = { kind = "optional", inner = { kind = "ref", name = "SessionOrigin" } } },
    { name = "restored", required = false, type = { kind = "boolean" } },
  },
}

M.types.StoredSession = {
  kind = "struct",
  fields = {
    { name = "summary", required = true, type = { kind = "ref", name = "SessionSummary" } },
    { name = "events", required = true, type = { kind = "list", item = { kind = "ref", name = "AgentEvent" } } },
    { name = "raw", required = true, type = { kind = "list", item = { kind = "ref", name = "RawLine" } } },
    { name = "working_directory", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
  },
}

--- A provider's own persisted session file, kept distinct from Styra's wire
--- journal. The text is verbatim JSONL, including records that never appeared
--- on the app-server wire.
M.types.ProviderRaw = {
  kind = "struct",
  fields = {
    { name = "provider", required = true, type = { kind = "ref", name = "Provider" } },
    { name = "text", required = true, type = { kind = "string" } },
  },
}

--- Host-side tmux endpoint for the shell owned by a live session's sandbox.
M.types.ShellInfo = {
  kind = "struct",
  fields = {
    { name = "tmux", required = true, type = { kind = "string", path = true } },
    { name = "socket", required = true, type = { kind = "string", path = true } },
  },
}

--- A `ProjectAction` started in a live session's sandbox: the tmux session
--- running it, beside the session's shell on the same server, for a client to
--- attach a terminal to. Its window stays open after the command exits, until
--- someone dismisses it, so a failure can still be read.
M.types.ProjectActionSession = {
  kind = "struct",
  fields = {
    { name = "tmux", required = true, type = { kind = "string", path = true } },
    { name = "socket", required = true, type = { kind = "string", path = true } },
    { name = "session", required = true, type = { kind = "string" } },
    { name = "command", required = true, type = { kind = "string" } },
  },
}

--- One turn's typed answer.
---
--- A reply that did not satisfy its contract is an `Answer` too, not an error
--- in place of one: `value` is absent, `error` says what was wrong, and
--- `source` still carries what the agent actually said. An agent that answered
--- well but framed it badly has produced something worth reading, and a client
--- that was handed only "no answer block" could not show it.
M.types.Answer = {
  kind = "struct",
  fields = {
    { name = "contract", required = true, type = { kind = "ref", name = "Contract" } },
    { name = "value", required = false, type = { kind = "optional", inner = { kind = "ref", name = "AnswerValue" } } },
    { name = "error", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "source", required = true, type = { kind = "string" } },
  },
}

--- One reading of a plan quota window, as a provider reported it mid-session.
---
--- Both interactive providers volunteer these unprompted, in different shapes
--- and with different amounts of detail — see `crate::quota`, which reads
--- them off the wire and keeps them.
M.types.QuotaEvent = {
  kind = "struct",
  fields = {
    { name = "at_ms", required = true, type = { kind = "number", integer = true } },
    { name = "session_id", required = true, type = { kind = "string" } },
    { name = "provider", required = true, type = { kind = "ref", name = "Provider" } },
    { name = "window", required = true, type = { kind = "string" } },
    { name = "status", required = true, type = { kind = "ref", name = "QuotaStatus" } },
    { name = "utilization", required = false, type = { kind = "optional", inner = { kind = "number" } } },
    { name = "resets_at_ms", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "detail", required = false, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- An append-only action lifecycle record. Records sharing `id` describe one
--- invocation. Sessions created by an action are linked by its terminal record.
M.types.ActionRecord = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "at_ms", required = true, type = { kind = "number", integer = true } },
    { name = "sessions", required = true, type = { kind = "list", item = { kind = "string" } } },
    { name = "action", required = true, type = { kind = "ref", name = "Action" } },
    { name = "origin", required = true, type = { kind = "ref", name = "ActionOrigin" } },
    { name = "status", required = true, type = { kind = "ref", name = "ActionStatus" } },
    { name = "detail", required = true, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- One process running inside a live interaction's sandbox, as the host's
--- `/proc` describes it at the moment of asking.
M.types.SandboxProcess = {
  kind = "struct",
  fields = {
    { name = "pid", required = true, type = { kind = "number", integer = true } },
    { name = "sandbox_pid", required = true, type = { kind = "number", integer = true } },
    { name = "parent_pid", required = true, type = { kind = "number", integer = true } },
    { name = "state", required = true, type = { kind = "string" } },
    { name = "command", required = true, type = { kind = "string" } },
    { name = "threads", required = true, type = { kind = "number", integer = true } },
    { name = "rss_kib", required = true, type = { kind = "number", integer = true } },
  },
}

--- What one turn changed in the checkout, read from its snapshots: see
--- `crate::protocol::Request::SnapshotTurnDiff`.
M.types.TurnDiff = {
  kind = "struct",
  fields = {
    { name = "diff", required = true, type = { kind = "string" } },
    { name = "ended", required = true, type = { kind = "boolean" } },
  },
}

--- What checkout diff mode compares with: see
--- `crate::protocol::Request::DiffBase`.
M.types.DiffBase = {
  kind = "struct",
  fields = {
    { name = "base", required = true, type = { kind = "string" } },
    { name = "branch", required = true, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- How much reasoning the model is asked to spend per turn.
---
--- One vocabulary across providers, ordered lowest first, since the ladders
--- coincide in the middle; `Provider::efforts_for` narrows it to what a given
--- model accepts. Passed to codex as its `model_reasoning_effort` config
--- override and to Claude Code as `--effort`.
M.types.Effort = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "minimal", payload = { kind = "unit" } },
    { name = "low", payload = { kind = "unit" } },
    { name = "medium", payload = { kind = "unit" } },
    { name = "high", payload = { kind = "unit" } },
    { name = "xhigh", payload = { kind = "unit" } },
    { name = "max", payload = { kind = "unit" } },
  },
}

--- One extra host directory the operator asked to be bound into the sandbox,
--- on top of what the profile and the selected templates already grant.
---
--- This is a *request*, not a resolved mount: the source is whatever the
--- operator typed, and the server canonicalizes it (rejecting a path that does
--- not exist) before it becomes part of a launch. An absent `destination`
--- means "the same path inside the sandbox", matching Driva's own rule for a
--- bind mount with no destination.
M.types.LaunchMount = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "source", required = true, type = { kind = "string", path = true } },
    { name = "destination", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
    { name = "writable", required = false, type = { kind = "boolean" } },
  },
}

--- The branch and commit a Session's own branch was created from, recorded
--- when Styra created it.
---
--- The commit is the one the branch was actually made at, not the one its
--- origin points to now: an origin branch moves on, and what the Session's
--- branch has that it did not is measured from here.
M.types.BranchPoint = {
  kind = "struct",
  fields = {
    { name = "branch", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "commit", required = true, type = { kind = "string" } },
  },
}

M.types.FileEditReport = {
  kind = "struct",
  fields = {
    { name = "path", required = true, type = { kind = "string" } },
    { name = "fingerprint", required = true, type = { kind = "string" } },
    { name = "description", required = true, type = { kind = "string" } },
  },
}

--- A resolved mount together with the layer that asked for it.
M.types.AttributedMount = {
  kind = "struct",
  fields = {
    { name = "origin", required = true, type = { kind = "ref", name = "MountOrigin" } },
    { name = "mount", required = true, type = { kind = "ref", name = "Mount" } },
  },
}

--- One capability of the sandbox's base system, as it resolved on this host.
---
--- A capability is the portable statement ("this sandbox can resolve host
--- names"); the paths are what that means on this machine. Reporting both lets
--- an operator see not only what the private root holds but why it holds it.
M.types.BaseCapability = {
  kind = "struct",
  fields = {
    { name = "name", required = true, type = { kind = "string" } },
    { name = "description", required = true, type = { kind = "string" } },
    { name = "entries", required = true, type = { kind = "list", item = { kind = "ref", name = "BaseEntry" } } },
    { name = "environment", required = false, type = { kind = "list", item = { kind = "string" } } },
  },
}

--- One part of the sandbox filesystem the backend lays down itself, for every
--- execution, whether or not any mount asked for it.
---
--- A mount is a grant an operator or a profile made; a floor entry is what the
--- backend needs there regardless — the root the mounts are laid on, the
--- `/proc` a process reads about itself through, the scratch space every
--- program assumes at `/tmp`. Nothing here exposes host content except the
--- prepared rootfs an execution was configured with, but some of these are
--- *writable*, and a caller that only reported the mounts would be saying the
--- sandbox holds less than it does.
---
--- Backends report these so a caller can state them (see
--- `BwrapIsolation::floor`); the same list is what the backend renders its
--- own invocation from, so what is shown and what is built cannot drift.
M.types.FloorEntry = {
  kind = "struct",
  fields = {
    { name = "kind", required = true, type = { kind = "ref", name = "FloorKind" } },
    { name = "path", required = true, type = { kind = "string", path = true } },
    { name = "source", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
  },
}

--- One environment variable the agent will run with, and where it came from.
M.types.AttributedVariable = {
  kind = "struct",
  fields = {
    { name = "origin", required = true, type = { kind = "ref", name = "VariableOrigin" } },
    { name = "name", required = true, type = { kind = "string" } },
    { name = "value", required = true, type = { kind = "string" } },
  },
}

M.types.WritableMountMode = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "direct", payload = { kind = "unit" } },
    { name = "overlay", payload = { kind = "unit" } },
  },
}

--- The outcome half of `CleanedWorktree`.
M.types.WorktreeCleanup = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "outcome", content = "data" },
  variants = {
    { name = "removed", payload = { kind = "unit" } },
    { name = "already_gone", payload = { kind = "unit" } },
    { name = "uncommitted", payload = { kind = "unit" } },
    { name = "live", payload = { kind = "unit" } },
    { name = "shared", payload = {
      kind = "struct",
      fields = {
        { name = "session_id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "failed", payload = {
      kind = "struct",
      fields = {
        { name = "message", required = true, type = { kind = "string" } },
      },
    } },
  },
}

--- One Session working in a `WorktreeSummary`.
M.types.WorktreeSession = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "completed", required = true, type = { kind = "ref", name = "CompletionState" } },
    { name = "live", required = true, type = { kind = "boolean" } },
  },
}

--- Where a Session came from, when it was not launched fresh but branched
--- from another one — see `crate::server::ServerState::branch_session`.
--- Recorded once, at branch time; it never updates as the source Session
--- keeps being worked on afterwards, the same way a git branch's fork point
--- does not move when the source gets new commits.
M.types.SessionOrigin = {
  kind = "struct",
  fields = {
    { name = "session_id", required = true, type = { kind = "string" } },
    { name = "provider", required = true, type = { kind = "ref", name = "Provider" } },
    { name = "at_ms", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "history", required = false, type = { kind = "ref", name = "BranchHistory" } },
  },
}

--- Where integration landed a Session's work: the squash commit it made on
--- the branch the Session's own branch was made from — see
--- `crate::protocol::Request::IntegrateSession`.
M.types.Integration = {
  kind = "struct",
  fields = {
    { name = "branch", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "commit", required = true, type = { kind = "string" } },
  },
}

M.types.SequencedUpdate = {
  kind = "struct",
  fields = {
    { name = "sequence", required = true, type = { kind = "number", integer = true } },
    { name = "update", required = true, type = { kind = "ref", name = "InteractionUpdate" } },
  },
}

--- Where an interaction is in its life: what its agent is doing, or that it is
--- doing nothing further.
---
--- One state rather than a state and a liveness flag beside it. Whether an
--- interaction still takes messages is not a second fact about it — it is the
--- difference between the first three of these and the last — and a client
--- that had to consult two fields to find out could be handed the pair that
--- says a stopped agent is waiting for input.
M.types.InteractionActivity = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "pending", payload = { kind = "unit" } },
    { name = "running", payload = { kind = "unit" } },
    { name = "background", payload = { kind = "unit" } },
    { name = "stopped", payload = { kind = "unit" } },
  },
}

--- How an interaction came to be where it is: what ended the turn it is not
--- working on, or what stopped it taking messages.
---
--- `InteractionActivity` says what an interaction is doing; it does not say
--- why, and the two questions have different answers. An interaction that
--- finished its turn, one the operator interrupted, one whose turn failed, and
--- one a plan window refused are all `Pending` — one word for four situations
--- an operator would act on differently — and every way of arriving at
--- `Stopped` looks alike from the outside.
---
--- Only the server can answer it. The reason is a fact about a moment that has
--- passed by the time anyone asks: the agent reports the same `turn/completed`
--- whether it ran its course or was cut off a second earlier, so a client that
--- was not watching when the interrupt went out has no way to reconstruct it —
--- and none of Styra's clients are watching all of the time.
M.types.InteractionActivityReason = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "reason", content = "detail" },
  variants = {
    { name = "turn_completed", payload = { kind = "unit" } },
    { name = "interrupted", payload = { kind = "unit" } },
    { name = "failed", payload = {
      kind = "struct",
      fields = {
        { name = "message", required = true, type = { kind = "string" } },
      },
    } },
    { name = "rate_limited", payload = {
      kind = "struct",
      fields = {
        { name = "window", required = true, type = { kind = "string" } },
        { name = "resets_at_ms", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
      },
    } },
    { name = "background_finished", payload = { kind = "unit" } },
    { name = "paused", payload = { kind = "unit" } },
    { name = "exited", payload = {
      kind = "struct",
      fields = {
        { name = "exit_code", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
      },
    } },
    { name = "server_restarted", payload = { kind = "unit" } },
    { name = "branched", payload = { kind = "unit" } },
  },
}

--- Where an interaction's agent is working, in Git's terms.
---
--- Read from Git rather than from the name Styra gave the checkout. A Session
--- records the checkout it was *given* (see `styra_server::worktree::Checkout`),
--- including changes made by retargeting; this says what is actually
--- there now — the agent may have switched branches, or the Workspace may be
--- the operator's own checkout that Styra never made anything for.
---
--- Located at launch and when the interaction goes idle or changes directory.
--- Live interactions also refresh the branch during polling, at most once
--- every two seconds, to detect switches made by the agent or operator.
M.types.CheckoutState = {
  kind = "struct",
  fields = {
    { name = "worktree", required = true, type = { kind = "string", path = true } },
    { name = "repository", required = true, type = { kind = "string", path = true } },
    { name = "branch", required = false, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- One message of an interaction's conversation, as
--- `InteractionSummary::recent_messages` lists them.
M.types.RecentMessage = {
  kind = "struct",
  fields = {
    { name = "from_operator", required = false, type = { kind = "boolean" } },
    { name = "text", required = true, type = { kind = "string" } },
    { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
    { name = "branch", required = false, type = { kind = "optional", inner = { kind = "ref", name = "RecentBranch" } } },
  },
}

--- The stable, provider-independent event vocabulary.
M.types.AgentEvent = {
  kind = "enum",
  tagging = { style = "internal", tag = "type" },
  variants = {
    { name = "user_message", payload = {
      kind = "struct",
      fields = {
        { name = "text", required = true, type = { kind = "string" } },
      },
    } },
    { name = "thread_started", payload = {
      kind = "struct",
      fields = {
        { name = "thread_id", required = true, type = { kind = "string" } },
        { name = "model", required = false, type = { kind = "optional", inner = { kind = "string" } } },
        { name = "effort", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "turn_started", payload = { kind = "unit" } },
    { name = "turn_completed", payload = {
      kind = "struct",
      fields = {
        { name = "outcome", required = false, type = { kind = "ref", name = "TurnOutcome" } },
        { name = "usage", required = true, type = { kind = "ref", name = "TurnUsage" } },
      },
    } },
    { name = "usage_updated", payload = {
      kind = "struct",
      fields = {
        { name = "usage", required = true, type = { kind = "ref", name = "TokenUsage" } },
      },
    } },
    { name = "command_started", payload = {
      kind = "struct",
      fields = {
        { name = "command", required = true, type = { kind = "string" } },
      },
    } },
    { name = "command_completed", payload = {
      kind = "struct",
      fields = {
        { name = "command", required = true, type = { kind = "string" } },
        { name = "status", required = true, type = { kind = "string" } },
        { name = "exit_code", required = true, type = { kind = "optional", inner = { kind = "number", integer = true } } },
        { name = "output", required = false, type = { kind = "string" } },
      },
    } },
    { name = "file_changed", payload = {
      kind = "struct",
      fields = {
        { name = "id", required = false, type = { kind = "string" } },
        { name = "paths", required = true, type = { kind = "list", item = { kind = "string" } } },
        { name = "diff", required = false, type = { kind = "optional", inner = { kind = "string" } } },
        { name = "checkpoint", required = false, type = { kind = "optional", inner = { kind = "string" } } },
        { name = "checkpoint_error", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "diff_updated", payload = {
      kind = "struct",
      fields = {
        { name = "diff", required = true, type = { kind = "string" } },
      },
    } },
    { name = "tool_started", payload = {
      kind = "struct",
      fields = {
        { name = "id", required = false, type = { kind = "string" } },
        { name = "name", required = true, type = { kind = "string" } },
        { name = "detail", required = true, type = { kind = "string" } },
      },
    } },
    { name = "tool_completed", payload = {
      kind = "struct",
      fields = {
        { name = "id", required = false, type = { kind = "string" } },
        { name = "name", required = true, type = { kind = "string" } },
        { name = "detail", required = false, type = { kind = "string" } },
        { name = "status", required = true, type = { kind = "string" } },
        { name = "output", required = false, type = { kind = "string" } },
      },
    } },
    { name = "plan_updated", payload = {
      kind = "struct",
      fields = {
        { name = "text", required = true, type = { kind = "string" } },
      },
    } },
    { name = "agent_message", payload = {
      kind = "struct",
      fields = {
        { name = "text", required = true, type = { kind = "string" } },
      },
    } },
    { name = "thinking", payload = {
      kind = "struct",
      fields = {
        { name = "text", required = true, type = { kind = "string" } },
        { name = "tokens", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
      },
    } },
    { name = "error", payload = {
      kind = "struct",
      fields = {
        { name = "message", required = true, type = { kind = "string" } },
      },
    } },
    { name = "model_changed", payload = {
      kind = "struct",
      fields = {
        { name = "model", required = false, type = { kind = "optional", inner = { kind = "string" } } },
        { name = "effort", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "provider_changed", payload = {
      kind = "struct",
      fields = {
        { name = "from", required = true, type = { kind = "string" } },
        { name = "to", required = true, type = { kind = "string" } },
        { name = "model", required = true, type = { kind = "string" } },
        { name = "effort", required = true, type = { kind = "string" } },
      },
    } },
    { name = "branched", payload = {
      kind = "struct",
      fields = {
        { name = "direction", required = true, type = { kind = "ref", name = "BranchDirection" } },
        { name = "session", required = true, type = { kind = "string" } },
        { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "task_started", payload = {
      kind = "struct",
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "description", required = true, type = { kind = "string" } },
        { name = "kind", required = true, type = { kind = "string" } },
        { name = "agent", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "task_progress", payload = {
      kind = "struct",
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "description", required = true, type = { kind = "string" } },
        { name = "agent", required = false, type = { kind = "optional", inner = { kind = "string" } } },
        { name = "tool", required = false, type = { kind = "optional", inner = { kind = "string" } } },
        { name = "tokens", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
      },
    } },
    { name = "task_completed", payload = {
      kind = "struct",
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "status", required = true, type = { kind = "string" } },
        { name = "summary", required = false, type = { kind = "string" } },
        { name = "error", required = false, type = { kind = "optional", inner = { kind = "string" } } },
      },
    } },
    { name = "background_tasks", payload = {
      kind = "struct",
      fields = {
        { name = "running", required = true, type = { kind = "number", integer = true } },
      },
    } },
    { name = "unknown", payload = {
      kind = "struct",
      fields = {
        { name = "wire_type", required = true, type = { kind = "string" } },
      },
    } },
    { name = "malformed", payload = {
      kind = "struct",
      fields = {
        { name = "error", required = true, type = { kind = "string" } },
      },
    } },
  },
}

--- One verbatim line of the agent interaction, undecoded.
M.types.RawLine = {
  kind = "struct",
  fields = {
    { name = "direction", required = true, type = { kind = "ref", name = "Direction" } },
    { name = "text", required = true, type = { kind = "string" } },
    { name = "at_ms", required = false, type = { kind = "number", integer = true } },
  },
}

--- A parsed answer. The variant always matches the `Contract` that produced
--- it, so a client can dispatch on the value alone.
M.types.AnswerValue = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "contract", content = "value" },
  variants = {
    { name = "text", payload = { kind = "newtype", type = { kind = "string" } } },
    { name = "lines", payload = { kind = "newtype", type = { kind = "list", item = { kind = "string" } } } },
    { name = "files", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "FileLocation" } } } },
    { name = "json", payload = { kind = "newtype", type = { kind = "any" } } },
  },
}

--- How much of a quota window is left, as the provider judges it.
M.types.QuotaStatus = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "allowed", payload = { kind = "unit" } },
    { name = "warning", payload = { kind = "unit" } },
    { name = "exhausted", payload = { kind = "unit" } },
  },
}

--- A server action, captured at the common dispatch boundary. The original
--- request preserves every input, including fields added by future operations.
M.types.Action = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "type", content = "data" },
  variants = {
    { name = "request", payload = { kind = "newtype", type = { kind = "ref", name = "Request" } } },
    { name = "commit_turn", payload = {
      kind = "struct",
      fields = {
        { name = "message", required = true, type = { kind = "string" } },
      },
    } },
  },
}

M.types.ActionOrigin = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "client", payload = { kind = "unit" } },
    { name = "automatic", payload = { kind = "unit" } },
  },
}

--- Started without a terminal record means the server stopped before it could
--- report an outcome; it must never be interpreted as success.
M.types.ActionStatus = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "started", payload = { kind = "unit" } },
    { name = "succeeded", payload = { kind = "unit" } },
    { name = "failed", payload = { kind = "unit" } },
  },
}

--- Which layer of the launch policy put a mount in the sandbox.
---
--- The effective policy is one flat list by the time Driva runs it, but the
--- layers that produced it are not interchangeable to an operator: a grant from
--- the agent profile is a property of the agent they picked, one from a
--- template is a property of the template, and one they typed is theirs to take
--- back. Recording the layer at the point the list is built is the only place
--- the answer is known for certain — afterwards a mount is just a path pair.
M.types.MountOrigin = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "workspace", payload = { kind = "unit" } },
    { name = "git_repository", payload = { kind = "unit" } },
    { name = "scratch", payload = { kind = "unit" } },
    { name = "profile", payload = { kind = "unit" } },
    { name = "template", payload = { kind = "unit" } },
    { name = "tooling", payload = { kind = "unit" } },
    { name = "operator", payload = { kind = "unit" } },
    { name = "broker", payload = { kind = "unit" } },
  },
}

--- A filesystem made available inside an isolated execution.
M.types.Mount = {
  kind = "enum",
  tagging = { style = "internal", tag = "kind" },
  variants = {
    { name = "bind", payload = {
      kind = "struct",
      fields = {
        { name = "source", required = true, type = { kind = "string", path = true } },
        { name = "destination", required = true, type = { kind = "string", path = true } },
        { name = "access", required = true, type = { kind = "ref", name = "MountAccess" } },
      },
    } },
    { name = "temporary", payload = {
      kind = "struct",
      fields = {
        { name = "destination", required = true, type = { kind = "string", path = true } },
      },
    } },
    { name = "overlay", payload = {
      kind = "struct",
      fields = {
        { name = "source", required = true, type = { kind = "string", path = true } },
        { name = "destination", required = true, type = { kind = "string", path = true } },
      },
    } },
  },
}

--- One path the base lays down, and where its content comes from when that is
--- not the same place (a host link followed out of the base).
M.types.BaseEntry = {
  kind = "struct",
  fields = {
    { name = "path", required = true, type = { kind = "string", path = true } },
    { name = "source", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
  },
}

--- What one `FloorEntry` is.
M.types.FloorKind = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "private-root", payload = { kind = "unit" } },
    { name = "tmpfs", payload = { kind = "unit" } },
    { name = "root-fs", payload = { kind = "unit" } },
    { name = "proc", payload = { kind = "unit" } },
    { name = "devices", payload = { kind = "unit" } },
    { name = "directory", payload = { kind = "unit" } },
  },
}

--- Which layer set one environment variable.
---
--- The same question the mount list answers, for the other half of what
--- crosses into the sandbox: a variable an operator does not recognize is
--- either the sandbox's own doing, a capability forwarding it from the host,
--- or something the profile, the operator's user config, a template or the
--- shell broker asked for.
M.types.VariableOrigin = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "sandbox", payload = { kind = "unit" } },
    { name = "base", payload = { kind = "unit" } },
    { name = "profile", payload = { kind = "unit" } },
    { name = "user", payload = { kind = "unit" } },
    { name = "template", payload = { kind = "unit" } },
    { name = "broker", payload = { kind = "unit" } },
  },
}

--- An update delivered from the interaction's threads to the UI.
M.types.InteractionUpdate = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "type", content = "data" },
  variants = {
    { name = "event", payload = { kind = "newtype", type = { kind = "ref", name = "AgentEvent" } } },
    { name = "raw", payload = { kind = "newtype", type = { kind = "ref", name = "RawLine" } } },
    { name = "log", payload = { kind = "newtype", type = { kind = "ref", name = "LogEntry" } } },
    { name = "quota", payload = { kind = "newtype", type = { kind = "ref", name = "QuotaEvent" } } },
    { name = "working_directory_changed", payload = { kind = "newtype", type = { kind = "string", path = true } } },
    { name = "ended", payload = { kind = "newtype", type = { kind = "ref", name = "InteractionEnd" } } },
  },
}

--- A branch marker in `InteractionSummary::recent_messages`: which way the
--- link points and the other Session's id. Its name is left for the client to
--- resolve live, as for the marker itself.
M.types.RecentBranch = {
  kind = "struct",
  fields = {
    { name = "direction", required = true, type = { kind = "ref", name = "BranchDirection" } },
    { name = "session", required = true, type = { kind = "string" } },
  },
}

--- How a turn ended, as far as the provider stated it.
---
--- A turn that failed is still a turn that ended, and both providers say so on
--- the line that ends it — Claude in the `result`'s `subtype` and `is_error`,
--- the app-server in `turn.status` and `turn.error`. Reading the failure as an
--- error and nothing else, which is what Claude's decoder used to do, left a
--- failed turn with no ending at all: no end-of-turn line in the log, and a
--- client waiting for a `TurnCompleted` that was never coming.
---
--- What it does not cover is why a turn ended *early* by the operator's own
--- doing. An interrupt looks like an ordinary ending on the wire, so only the
--- client that asked for it knows.
M.types.TurnOutcome = {
  kind = "enum",
  tagging = { style = "internal", tag = "outcome" },
  variants = {
    { name = "completed", payload = { kind = "unit" } },
    { name = "failed", payload = {
      kind = "struct",
      fields = {
        { name = "message", required = true, type = { kind = "string" } },
      },
    } },
  },
}

--- What a turn cost, and what the thread has cost through the end of it.
---
--- No provider reports both, and they do not report the same one: the
--- app-server sends a running thread total in its own notification and
--- nothing at all with the turn's end, while Claude and `codex exec` report
--- the turn's own spend and never a total. Each decoder fills only the half
--- its wire line actually states — an absent figure stays `None` rather than
--- becoming a zero that reads like a real measurement — and `UsageTracker`
--- derives the other half from the run of events around it.
---
--- Both halves are optional, so this accepts almost anything — including the
--- bare `TokenUsage` that used to sit in `usage`'s place, which would come
--- through as neither figure and drop what a peer on an older build actually
--- said. `deny_unknown_fields` is what makes that a decode error instead of a
--- silent loss: a client showing nothing where a number was reported is worse
--- than one saying it could not read the line.
M.types.TurnUsage = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "turn", required = false, type = { kind = "optional", inner = { kind = "ref", name = "TokenUsage" } } },
    { name = "total", required = false, type = { kind = "optional", inner = { kind = "ref", name = "TokenUsage" } } },
  },
}

M.types.TokenUsage = {
  kind = "struct",
  fields = {
    { name = "input_tokens", required = false, type = { kind = "number", integer = true } },
    { name = "cached_input_tokens", required = false, type = { kind = "number", integer = true } },
    { name = "output_tokens", required = false, type = { kind = "number", integer = true } },
    { name = "reasoning_output_tokens", required = false, type = { kind = "number", integer = true } },
  },
}

--- Which side of a branch an `AgentEvent::Branched` marker sits on. Each
--- marker names the other session, so the direction says which way the link
--- points rather than which session it belongs to.
M.types.BranchDirection = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "from", payload = { kind = "unit" } },
    { name = "to", payload = { kind = "unit" } },
  },
}

--- Which way a wire line travelled.
M.types.Direction = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "to_agent", payload = { kind = "unit" } },
    { name = "from_agent", payload = { kind = "unit" } },
  },
}

--- A place in the Workspace, as named by a `Contract::Files` answer.
---
--- `line` and `column` are 1-based and `None` when the agent named none, which
--- is the difference between "this file" and "this position", and is why they
--- are not zero-defaulted into a position that does not exist.
M.types.FileLocation = {
  kind = "struct",
  fields = {
    { name = "path", required = true, type = { kind = "string", path = true } },
    { name = "line", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "column", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "end_line", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "description", required = false, type = { kind = "string" } },
  },
}

M.types.MountAccess = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "readonly", payload = { kind = "unit" } },
    { name = "readwrite", payload = { kind = "unit" } },
  },
}

--- One line in the log view: a Styra-internal note or a line of agent stderr.
M.types.LogEntry = {
  kind = "struct",
  fields = {
    { name = "level", required = true, type = { kind = "ref", name = "LogLevel" } },
    { name = "message", required = true, type = { kind = "string" } },
  },
}

--- How an interaction finished.
M.types.InteractionEnd = {
  kind = "struct",
  fields = {
    { name = "exit_code", required = true, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "error", required = true, type = { kind = "optional", inner = { kind = "string" } } },
  },
}

--- Severity of a `LogEntry`, used to colour the log view.
M.types.LogLevel = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "info", payload = { kind = "unit" } },
    { name = "warn", payload = { kind = "unit" } },
    { name = "error", payload = { kind = "unit" } },
  },
}

-- Enum spellings
-- --------------

--- The wire spellings of every enum, in declaration order.
M.enums = {}

M.enums.Request = { "integrate_session", "generate_integration_message", "generate_edit_report", "edit_report", "session_integration_result", "rebase_session", "retarget_session", "session_branch_history", "restore_session_branch", "session_rebase_status", "health", "create_workspace", "list_workspaces", "list_deleted_workspaces", "delete_workspace", "restore_workspace", "workspace", "workspace_for_path", "rename_workspace", "set_workspace_git_repository", "set_workspace_host_path", "workspace_launch", "create_session", "plan_session", "list_templates", "list_models", "remember_model", "resume_session", "create_session_worktree", "clean_worktrees", "list_worktrees", "convert_session_provider", "branch_session", "rename_session", "set_session_tags", "list_tags", "session_composer", "set_session_composer", "transcribe_audio", "audio_recording_started", "audio_recording_stopped", "audio_transcription_error", "change_workspace_launch", "send_message", "set_session_selection", "set_interaction_working_directory", "set_interaction_auto_retry", "set_interaction_auto_commit", "queue_message", "send_queued_message", "clear_queued_messages", "interrupt_interaction", "stop_interaction", "set_session_completed", "close_interaction", "load_interaction", "updates", "list_interactions", "list_sessions", "stored_session", "provider_raw", "shell", "start_project_action", "turn_answer", "quota_log", "interaction_actions", "interaction_processes", "interaction_driva", "diff_base", "snapshot_turn_diff", "shutdown" }
--- Wire spellings of `Request`.
M.Request = {
  INTEGRATE_SESSION = "integrate_session",
  GENERATE_INTEGRATION_MESSAGE = "generate_integration_message",
  GENERATE_EDIT_REPORT = "generate_edit_report",
  EDIT_REPORT = "edit_report",
  SESSION_INTEGRATION_RESULT = "session_integration_result",
  REBASE_SESSION = "rebase_session",
  RETARGET_SESSION = "retarget_session",
  SESSION_BRANCH_HISTORY = "session_branch_history",
  RESTORE_SESSION_BRANCH = "restore_session_branch",
  SESSION_REBASE_STATUS = "session_rebase_status",
  HEALTH = "health",
  CREATE_WORKSPACE = "create_workspace",
  LIST_WORKSPACES = "list_workspaces",
  LIST_DELETED_WORKSPACES = "list_deleted_workspaces",
  DELETE_WORKSPACE = "delete_workspace",
  RESTORE_WORKSPACE = "restore_workspace",
  WORKSPACE = "workspace",
  WORKSPACE_FOR_PATH = "workspace_for_path",
  RENAME_WORKSPACE = "rename_workspace",
  SET_WORKSPACE_GIT_REPOSITORY = "set_workspace_git_repository",
  SET_WORKSPACE_HOST_PATH = "set_workspace_host_path",
  WORKSPACE_LAUNCH = "workspace_launch",
  CREATE_SESSION = "create_session",
  PLAN_SESSION = "plan_session",
  LIST_TEMPLATES = "list_templates",
  LIST_MODELS = "list_models",
  REMEMBER_MODEL = "remember_model",
  RESUME_SESSION = "resume_session",
  CREATE_SESSION_WORKTREE = "create_session_worktree",
  CLEAN_WORKTREES = "clean_worktrees",
  LIST_WORKTREES = "list_worktrees",
  CONVERT_SESSION_PROVIDER = "convert_session_provider",
  BRANCH_SESSION = "branch_session",
  RENAME_SESSION = "rename_session",
  SET_SESSION_TAGS = "set_session_tags",
  LIST_TAGS = "list_tags",
  SESSION_COMPOSER = "session_composer",
  SET_SESSION_COMPOSER = "set_session_composer",
  TRANSCRIBE_AUDIO = "transcribe_audio",
  AUDIO_RECORDING_STARTED = "audio_recording_started",
  AUDIO_RECORDING_STOPPED = "audio_recording_stopped",
  AUDIO_TRANSCRIPTION_ERROR = "audio_transcription_error",
  CHANGE_WORKSPACE_LAUNCH = "change_workspace_launch",
  SEND_MESSAGE = "send_message",
  SET_SESSION_SELECTION = "set_session_selection",
  SET_INTERACTION_WORKING_DIRECTORY = "set_interaction_working_directory",
  SET_INTERACTION_AUTO_RETRY = "set_interaction_auto_retry",
  SET_INTERACTION_AUTO_COMMIT = "set_interaction_auto_commit",
  QUEUE_MESSAGE = "queue_message",
  SEND_QUEUED_MESSAGE = "send_queued_message",
  CLEAR_QUEUED_MESSAGES = "clear_queued_messages",
  INTERRUPT_INTERACTION = "interrupt_interaction",
  STOP_INTERACTION = "stop_interaction",
  SET_SESSION_COMPLETED = "set_session_completed",
  CLOSE_INTERACTION = "close_interaction",
  LOAD_INTERACTION = "load_interaction",
  UPDATES = "updates",
  LIST_INTERACTIONS = "list_interactions",
  LIST_SESSIONS = "list_sessions",
  STORED_SESSION = "stored_session",
  PROVIDER_RAW = "provider_raw",
  SHELL = "shell",
  START_PROJECT_ACTION = "start_project_action",
  TURN_ANSWER = "turn_answer",
  QUOTA_LOG = "quota_log",
  INTERACTION_ACTIONS = "interaction_actions",
  INTERACTION_PROCESSES = "interaction_processes",
  INTERACTION_DRIVA = "interaction_driva",
  DIFF_BASE = "diff_base",
  SNAPSHOT_TURN_DIFF = "snapshot_turn_diff",
  SHUTDOWN = "shutdown",
}

M.enums.Response = { "rebase_started", "session_branch_history", "session_rebase_status", "integration_started", "integration_message", "edit_report", "session_integration_result", "health", "workspace_created", "workspaces", "workspace", "workspace_for_path", "workspace_renamed", "workspace_git_repository_updated", "workspace_host_path_updated", "workspace_launch", "session_created", "session_plan", "templates", "models", "model_remembered", "session_resumed", "session_worktree_created", "worktrees_cleaned", "worktrees", "session_converted", "session_branched", "session_renamed", "session_tags_updated", "tags", "session_composer", "session_composer_stored", "workspace_launch_updated", "accepted", "queued", "sent_queued_message", "queued_messages", "interaction_loaded", "updates", "interactions", "stored_sessions", "stored_session", "provider_raw", "audio_transcript", "shell", "project_action_started", "answer", "quota_log", "interaction_actions", "interaction_processes", "interaction_driva", "snapshot_turn_diff", "diff_base" }
--- Wire spellings of `Response`.
M.Response = {
  REBASE_STARTED = "rebase_started",
  SESSION_BRANCH_HISTORY = "session_branch_history",
  SESSION_REBASE_STATUS = "session_rebase_status",
  INTEGRATION_STARTED = "integration_started",
  INTEGRATION_MESSAGE = "integration_message",
  EDIT_REPORT = "edit_report",
  SESSION_INTEGRATION_RESULT = "session_integration_result",
  HEALTH = "health",
  WORKSPACE_CREATED = "workspace_created",
  WORKSPACES = "workspaces",
  WORKSPACE = "workspace",
  WORKSPACE_FOR_PATH = "workspace_for_path",
  WORKSPACE_RENAMED = "workspace_renamed",
  WORKSPACE_GIT_REPOSITORY_UPDATED = "workspace_git_repository_updated",
  WORKSPACE_HOST_PATH_UPDATED = "workspace_host_path_updated",
  WORKSPACE_LAUNCH = "workspace_launch",
  SESSION_CREATED = "session_created",
  SESSION_PLAN = "session_plan",
  TEMPLATES = "templates",
  MODELS = "models",
  MODEL_REMEMBERED = "model_remembered",
  SESSION_RESUMED = "session_resumed",
  SESSION_WORKTREE_CREATED = "session_worktree_created",
  WORKTREES_CLEANED = "worktrees_cleaned",
  WORKTREES = "worktrees",
  SESSION_CONVERTED = "session_converted",
  SESSION_BRANCHED = "session_branched",
  SESSION_RENAMED = "session_renamed",
  SESSION_TAGS_UPDATED = "session_tags_updated",
  TAGS = "tags",
  SESSION_COMPOSER = "session_composer",
  SESSION_COMPOSER_STORED = "session_composer_stored",
  WORKSPACE_LAUNCH_UPDATED = "workspace_launch_updated",
  ACCEPTED = "accepted",
  QUEUED = "queued",
  SENT_QUEUED_MESSAGE = "sent_queued_message",
  QUEUED_MESSAGES = "queued_messages",
  INTERACTION_LOADED = "interaction_loaded",
  UPDATES = "updates",
  INTERACTIONS = "interactions",
  STORED_SESSIONS = "stored_sessions",
  STORED_SESSION = "stored_session",
  PROVIDER_RAW = "provider_raw",
  AUDIO_TRANSCRIPT = "audio_transcript",
  SHELL = "shell",
  PROJECT_ACTION_STARTED = "project_action_started",
  ANSWER = "answer",
  QUOTA_LOG = "quota_log",
  INTERACTION_ACTIONS = "interaction_actions",
  INTERACTION_PROCESSES = "interaction_processes",
  INTERACTION_DRIVA = "interaction_driva",
  SNAPSHOT_TURN_DIFF = "snapshot_turn_diff",
  DIFF_BASE = "diff_base",
}

M.enums.WireResponse = { "ok", "error" }
--- Wire spellings of `WireResponse`.
M.WireResponse = {
  OK = "ok",
  ERROR = "error",
}

M.enums.BranchHistory = { "through_selected", "selected_only" }
--- Wire spellings of `BranchHistory`.
M.BranchHistory = {
  THROUGH_SELECTED = "through_selected",
  SELECTED_ONLY = "selected_only",
}

M.enums.Provider = { "codex", "claude" }
--- Wire spellings of `Provider`.
M.Provider = {
  CODEX = "codex",
  CLAUDE = "claude",
}

M.enums.WorkspaceLaunchChange = { "set_network", "set_writable_workspace", "set_templates", "add_mounts", "remove_mount", "replace", "set_command" }
--- Wire spellings of `WorkspaceLaunchChange`.
M.WorkspaceLaunchChange = {
  SET_NETWORK = "set_network",
  SET_WRITABLE_WORKSPACE = "set_writable_workspace",
  SET_TEMPLATES = "set_templates",
  ADD_MOUNTS = "add_mounts",
  REMOVE_MOUNT = "remove_mount",
  REPLACE = "replace",
  SET_COMMAND = "set_command",
}

M.enums.CompletionState = { "active", "completed", "abandoned", "sealed" }
--- Wire spellings of `CompletionState`.
M.CompletionState = {
  ACTIVE = "active",
  COMPLETED = "completed",
  ABANDONED = "abandoned",
  SEALED = "sealed",
}

M.enums.ProjectAction = { "run", "test" }
--- Wire spellings of `ProjectAction`.
M.ProjectAction = {
  RUN = "run",
  TEST = "test",
}

M.enums.Contract = { "text", "lines", "files", "json" }
--- Wire spellings of `Contract`.
M.Contract = {
  TEXT = "text",
  LINES = "lines",
  FILES = "files",
  JSON = "json",
}

M.enums.Effort = { "minimal", "low", "medium", "high", "xhigh", "max" }
--- Wire spellings of `Effort`.
M.Effort = {
  MINIMAL = "minimal",
  LOW = "low",
  MEDIUM = "medium",
  HIGH = "high",
  X_HIGH = "xhigh",
  MAX = "max",
}

M.enums.WritableMountMode = { "direct", "overlay" }
--- Wire spellings of `WritableMountMode`.
M.WritableMountMode = {
  DIRECT = "direct",
  OVERLAY = "overlay",
}

M.enums.WorktreeCleanup = { "removed", "already_gone", "uncommitted", "live", "shared", "failed" }
--- Wire spellings of `WorktreeCleanup`.
M.WorktreeCleanup = {
  REMOVED = "removed",
  ALREADY_GONE = "already_gone",
  UNCOMMITTED = "uncommitted",
  LIVE = "live",
  SHARED = "shared",
  FAILED = "failed",
}

M.enums.InteractionActivity = { "pending", "running", "background", "stopped" }
--- Wire spellings of `InteractionActivity`.
M.InteractionActivity = {
  PENDING = "pending",
  RUNNING = "running",
  BACKGROUND = "background",
  STOPPED = "stopped",
}

M.enums.InteractionActivityReason = { "turn_completed", "interrupted", "failed", "rate_limited", "background_finished", "paused", "exited", "server_restarted", "branched" }
--- Wire spellings of `InteractionActivityReason`.
M.InteractionActivityReason = {
  TURN_COMPLETED = "turn_completed",
  INTERRUPTED = "interrupted",
  FAILED = "failed",
  RATE_LIMITED = "rate_limited",
  BACKGROUND_FINISHED = "background_finished",
  PAUSED = "paused",
  EXITED = "exited",
  SERVER_RESTARTED = "server_restarted",
  BRANCHED = "branched",
}

M.enums.AgentEvent = { "user_message", "thread_started", "turn_started", "turn_completed", "usage_updated", "command_started", "command_completed", "file_changed", "diff_updated", "tool_started", "tool_completed", "plan_updated", "agent_message", "thinking", "error", "model_changed", "provider_changed", "branched", "task_started", "task_progress", "task_completed", "background_tasks", "unknown", "malformed" }
--- Wire spellings of `AgentEvent`.
M.AgentEvent = {
  USER_MESSAGE = "user_message",
  THREAD_STARTED = "thread_started",
  TURN_STARTED = "turn_started",
  TURN_COMPLETED = "turn_completed",
  USAGE_UPDATED = "usage_updated",
  COMMAND_STARTED = "command_started",
  COMMAND_COMPLETED = "command_completed",
  FILE_CHANGED = "file_changed",
  DIFF_UPDATED = "diff_updated",
  TOOL_STARTED = "tool_started",
  TOOL_COMPLETED = "tool_completed",
  PLAN_UPDATED = "plan_updated",
  AGENT_MESSAGE = "agent_message",
  THINKING = "thinking",
  ERROR = "error",
  MODEL_CHANGED = "model_changed",
  PROVIDER_CHANGED = "provider_changed",
  BRANCHED = "branched",
  TASK_STARTED = "task_started",
  TASK_PROGRESS = "task_progress",
  TASK_COMPLETED = "task_completed",
  BACKGROUND_TASKS = "background_tasks",
  UNKNOWN = "unknown",
  MALFORMED = "malformed",
}

M.enums.AnswerValue = { "text", "lines", "files", "json" }
--- Wire spellings of `AnswerValue`.
M.AnswerValue = {
  TEXT = "text",
  LINES = "lines",
  FILES = "files",
  JSON = "json",
}

M.enums.QuotaStatus = { "allowed", "warning", "exhausted" }
--- Wire spellings of `QuotaStatus`.
M.QuotaStatus = {
  ALLOWED = "allowed",
  WARNING = "warning",
  EXHAUSTED = "exhausted",
}

M.enums.Action = { "request", "commit_turn" }
--- Wire spellings of `Action`.
M.Action = {
  REQUEST = "request",
  COMMIT_TURN = "commit_turn",
}

M.enums.ActionOrigin = { "client", "automatic" }
--- Wire spellings of `ActionOrigin`.
M.ActionOrigin = {
  CLIENT = "client",
  AUTOMATIC = "automatic",
}

M.enums.ActionStatus = { "started", "succeeded", "failed" }
--- Wire spellings of `ActionStatus`.
M.ActionStatus = {
  STARTED = "started",
  SUCCEEDED = "succeeded",
  FAILED = "failed",
}

M.enums.MountOrigin = { "workspace", "git_repository", "scratch", "profile", "template", "tooling", "operator", "broker" }
--- Wire spellings of `MountOrigin`.
M.MountOrigin = {
  WORKSPACE = "workspace",
  GIT_REPOSITORY = "git_repository",
  SCRATCH = "scratch",
  PROFILE = "profile",
  TEMPLATE = "template",
  TOOLING = "tooling",
  OPERATOR = "operator",
  BROKER = "broker",
}

M.enums.Mount = { "bind", "temporary", "overlay" }
--- Wire spellings of `Mount`.
M.Mount = {
  BIND = "bind",
  TEMPORARY = "temporary",
  OVERLAY = "overlay",
}

M.enums.FloorKind = { "private-root", "tmpfs", "root-fs", "proc", "devices", "directory" }
--- Wire spellings of `FloorKind`.
M.FloorKind = {
  PRIVATE_ROOT = "private-root",
  TMPFS = "tmpfs",
  ROOT_FS = "root-fs",
  PROC = "proc",
  DEVICES = "devices",
  DIRECTORY = "directory",
}

M.enums.VariableOrigin = { "sandbox", "base", "profile", "user", "template", "broker" }
--- Wire spellings of `VariableOrigin`.
M.VariableOrigin = {
  SANDBOX = "sandbox",
  BASE = "base",
  PROFILE = "profile",
  USER = "user",
  TEMPLATE = "template",
  BROKER = "broker",
}

M.enums.InteractionUpdate = { "event", "raw", "log", "quota", "working_directory_changed", "ended" }
--- Wire spellings of `InteractionUpdate`.
M.InteractionUpdate = {
  EVENT = "event",
  RAW = "raw",
  LOG = "log",
  QUOTA = "quota",
  WORKING_DIRECTORY_CHANGED = "working_directory_changed",
  ENDED = "ended",
}

M.enums.TurnOutcome = { "completed", "failed" }
--- Wire spellings of `TurnOutcome`.
M.TurnOutcome = {
  COMPLETED = "completed",
  FAILED = "failed",
}

M.enums.BranchDirection = { "from", "to" }
--- Wire spellings of `BranchDirection`.
M.BranchDirection = {
  FROM = "from",
  TO = "to",
}

M.enums.Direction = { "to_agent", "from_agent" }
--- Wire spellings of `Direction`.
M.Direction = {
  TO_AGENT = "to_agent",
  FROM_AGENT = "from_agent",
}

M.enums.MountAccess = { "readonly", "readwrite" }
--- Wire spellings of `MountAccess`.
M.MountAccess = {
  READ_ONLY = "readonly",
  READ_WRITE = "readwrite",
}

M.enums.LogLevel = { "info", "warn", "error" }
--- Wire spellings of `LogLevel`.
M.LogLevel = {
  INFO = "info",
  WARN = "warn",
  ERROR = "error",
}

--- Every operation the server answers, in protocol order.
M.OPERATIONS = M.enums.Request

-- Runtime
-- -------

-- Hand-written support the generated tables are useless without: a null that
-- Lua does not have, a validator driven by `M.types`, and the two halves of a
-- request/response exchange. Everything here is generic over the descriptors;
-- it says nothing about any particular operation, so it does not need
-- regenerating when the protocol gains one.

--- The protocol's explicit null, for fields that mean something when set to it
--- (clearing a Session name, disassociating a Git repository). Lua cannot tell
--- an absent key from one set to nil, so a sentinel is the only way to say it.
--- Point this at your JSON encoder's own null before building any request:
---
---     protocol.use_null(cjson.null)
M.null = setmetatable({}, {
  __tostring = function()
    return "null"
  end,
})

--- Adopt `sentinel` as the value meaning JSON null, and return it.
function M.use_null(sentinel)
  M.null = sentinel
  return M.null
end

local function path_of(path, key)
  if path == "" then
    return tostring(key)
  end
  return path .. "." .. tostring(key)
end

local function fail(path, message, ...)
  if select("#", ...) > 0 then
    message = string.format(message, ...)
  end
  if path == "" then
    return false, message
  end
  return false, path .. ": " .. message
end

local function quoted_names(variants)
  local names = {}
  for index, variant in ipairs(variants) do
    names[index] = string.format("%q", variant.name)
  end
  return table.concat(names, ", ")
end

local function variant_named(descriptor, name)
  for _, variant in ipairs(descriptor.variants) do
    if variant.name == name then
      return variant
    end
  end
  return nil
end

local check_value, check_fields, check_payload

--- Check a table against a field list. `reserved` names keys that belong to
--- the encoding rather than to the fields (an internal tag sitting alongside
--- them), so they do not read as unknown.
function check_fields(fields, value, path, deny_unknown, reserved)
  if type(value) ~= "table" then
    return fail(path, "expected a table, got %s", type(value))
  end
  local known = {}
  for name in pairs(reserved or {}) do
    known[name] = true
  end
  for _, field in ipairs(fields) do
    known[field.name] = true
    local given = value[field.name]
    if given == nil then
      if field.required then
        return fail(path, "missing required field %q", field.name)
      end
    else
      local ok, err = check_value(field.type, given, path_of(path, field.name))
      if not ok then
        return false, err
      end
    end
  end
  if deny_unknown then
    for key in pairs(value) do
      if not known[key] then
        return fail(path, "unknown field %q", tostring(key))
      end
    end
  end
  return true
end

--- Whether a payload is one `Option` value, which may be left out entirely:
--- the operation then reads as if it carried `null`. A Lua `nil` cannot be
--- told apart from a missing argument, so that is how one is sent.
local function optional_newtype(payload)
  return payload.kind == "newtype" and payload.type.kind == "optional"
end

function check_payload(variant, content, path, deny_unknown)
  local payload = variant.payload
  if payload.kind == "unit" then
    if content ~= nil then
      return fail(path, "%q carries no data", variant.name)
    end
    return true
  end
  if content == nil then
    -- serde reads a missing newtype payload as its `None`.
    if optional_newtype(payload) then
      return true
    end
    return fail(path, "%q needs its data", variant.name)
  end
  if payload.kind == "newtype" then
    return check_value(payload.type, content, path)
  end
  if payload.kind == "tuple" then
    if type(content) ~= "table" then
      return fail(path, "expected a list of %d values, got %s", #payload.items, type(content))
    end
    for index, item in ipairs(payload.items) do
      local ok, err = check_value(item, content[index], path_of(path, index))
      if not ok then
        return false, err
      end
    end
    return true
  end
  return check_fields(payload.fields, content, path, payload.deny_unknown_fields or deny_unknown)
end

local function check_enum(descriptor, value, path)
  local tagging = descriptor.tagging
  if tagging.style == "untagged" then
    return true
  end
  if descriptor.plain then
    if type(value) ~= "string" then
      return fail(path, "expected one of %s, got %s", quoted_names(descriptor.variants), type(value))
    end
    if not variant_named(descriptor, value) then
      return fail(path, "%q is not one of %s", value, quoted_names(descriptor.variants))
    end
    return true
  end
  if tagging.style == "external" then
    if type(value) == "string" then
      local variant = variant_named(descriptor, value)
      if not variant then
        return fail(path, "%q is not one of %s", value, quoted_names(descriptor.variants))
      end
      return check_payload(variant, nil, path, descriptor.deny_unknown_fields)
    end
    if type(value) ~= "table" then
      return fail(path, "expected a table or a string, got %s", type(value))
    end
    local name, content = next(value)
    if name == nil then
      return fail(path, "names no variant; expected one of %s", quoted_names(descriptor.variants))
    end
    if next(value, name) ~= nil then
      return fail(path, "names more than one variant")
    end
    local variant = variant_named(descriptor, name)
    if not variant then
      return fail(path, "%q is not one of %s", tostring(name), quoted_names(descriptor.variants))
    end
    return check_payload(variant, content, path_of(path, name), descriptor.deny_unknown_fields)
  end
  if type(value) ~= "table" then
    return fail(path, "expected a table, got %s", type(value))
  end
  local name = value[tagging.tag]
  if type(name) ~= "string" then
    return fail(path, "has no %q naming one of %s", tagging.tag, quoted_names(descriptor.variants))
  end
  local variant = variant_named(descriptor, name)
  if not variant then
    return fail(path, "%q is not one of %s", name, quoted_names(descriptor.variants))
  end
  if tagging.style == "adjacent" then
    local content = value[tagging.content]
    local ok, err = check_payload(variant, content, path_of(path, tagging.content), descriptor.deny_unknown_fields)
    if not ok then
      return false, err
    end
    if descriptor.deny_unknown_fields then
      for key in pairs(value) do
        if key ~= tagging.tag and key ~= tagging.content then
          return fail(path, "unknown field %q", tostring(key))
        end
      end
    end
    return true
  end
  -- Internally tagged: the payload's fields sit beside the tag.
  if variant.payload.kind == "newtype" then
    return check_value(variant.payload.type, value, path)
  end
  local fields = variant.payload.kind == "struct" and variant.payload.fields or {}
  return check_fields(fields, value, path, descriptor.deny_unknown_fields, { [tagging.tag] = true })
end

function check_value(shape, value, path)
  local kind = shape.kind
  if kind == "optional" then
    if value == nil or value == M.null then
      return true
    end
    return check_value(shape.inner, value, path)
  end
  if value == M.null then
    return fail(path, "is not nullable")
  end
  if kind == "any" then
    return true
  end
  if kind == "string" then
    if type(value) ~= "string" then
      return fail(path, "expected a string, got %s", type(value))
    end
    return true
  end
  if kind == "number" then
    if type(value) ~= "number" then
      return fail(path, "expected a number, got %s", type(value))
    end
    if shape.integer and value % 1 ~= 0 then
      return fail(path, "expected a whole number, got %s", tostring(value))
    end
    return true
  end
  if kind == "boolean" then
    if type(value) ~= "boolean" then
      return fail(path, "expected a boolean, got %s", type(value))
    end
    return true
  end
  if kind == "list" then
    if type(value) ~= "table" then
      return fail(path, "expected a list, got %s", type(value))
    end
    for index, item in ipairs(value) do
      local ok, err = check_value(shape.item, item, path_of(path, index))
      if not ok then
        return false, err
      end
    end
    return true
  end
  if kind == "map" then
    if type(value) ~= "table" then
      return fail(path, "expected a table, got %s", type(value))
    end
    for key, item in pairs(value) do
      if type(key) ~= "string" then
        return fail(path, "has a non-string key")
      end
      local ok, err = check_value(shape.value, item, path_of(path, key))
      if not ok then
        return false, err
      end
    end
    return true
  end
  if kind == "ref" then
    local descriptor = M.types[shape.name]
    if not descriptor then
      return fail(path, "refers to unknown wire type %q", tostring(shape.name))
    end
    if descriptor.kind == "struct" then
      return check_fields(descriptor.fields, value, path, descriptor.deny_unknown_fields)
    end
    return check_enum(descriptor, value, path)
  end
  return fail(path, "has no shape the generator understands (%s)", tostring(kind))
end

--- Check a value against a named wire type: `true`, or `false` and a message
--- naming the field that was wrong.
---
---     local ok, err = protocol.validate("CreateSession", data)
function M.validate(name, value)
  if not M.types[name] then
    return false, string.format("unknown wire type %q", tostring(name))
  end
  return check_value({ kind = "ref", name = name }, value, "")
end

--- Build the request for `operation` from `data`, checking it first.
---
--- Raises on a request the server would refuse — a missing field, a misspelled
--- one, a value of the wrong shape — because a client that sends one gets a
--- socket round trip and an error string back instead of an answer, far from
--- the line that made the mistake. `M.validate` is the same check without the
--- raise, for a client that would rather show the message than fail.
function M.build(operation, data)
  local variant = variant_named(M.types.Request, operation)
  if not variant then
    error(string.format("%q is not a Styra operation", tostring(operation)), 2)
  end
  local payload = variant.payload
  if payload.kind == "unit" then
    if data ~= nil then
      error(string.format("%s takes no data", operation), 2)
    end
    return { operation = operation }
  end
  if data == nil then
    if optional_newtype(payload) then
      return { operation = operation }
    end
    error(string.format("%s needs its data", operation), 2)
  end
  local ok, err = check_payload(variant, data, "data", M.types.Request.deny_unknown_fields)
  if not ok then
    error(string.format("%s: %s", operation, err), 2)
  end
  return { operation = operation, data = data }
end

--- Read a decoded `WireResponse`: the response table (`type` and `data`), or
--- nil and the server's error message.
---
---     local response, err = protocol.unwrap(json.decode(line))
function M.unwrap(wire)
  if type(wire) ~= "table" then
    return nil, string.format("the server's reply is not an object (%s)", type(wire))
  end
  if wire.status == "ok" then
    return wire.response
  end
  if wire.status == "error" then
    return nil, wire.error or "the server reported an error with no message"
  end
  return nil, string.format("the server's reply has no status (%s)", tostring(wire.status))
end

--- The data of a response of the expected `type`, or nil and a message. Saves
--- every caller the same two checks: that the request succeeded, and that the
--- reply is about what was asked.
---
---     local health, err = protocol.expect(response, protocol.Response.HEALTH)
function M.expect(response, kind)
  local wire, err = M.unwrap(response)
  if not wire then
    return nil, err
  end
  if wire.type ~= kind then
    return nil, string.format("expected a %q response, got %q", tostring(kind), tostring(wire.type))
  end
  return wire.data == nil and true or wire.data
end

-- Requests
-- --------

--- One constructor per operation. Each checks what it is given and returns
--- the request table to encode and send.
M.request = {}

--- Rebase and squash-integrate a Session into its recorded source branch.
---
--- Fields of `data`:
---   id       string
---   message  string
function M.request.integrate_session(data)
  return M.build("integrate_session", data)
end

--- Ask an errand for an editable squash commit message.
---
--- Fields of `data`:
---   id  string
function M.request.generate_integration_message(data)
  return M.build("generate_integration_message", data)
end

--- Have an errand describe each edited file of the Session's checkout —
--- what changed in it and why — and keep the report with the Session,
--- replacing any earlier one. The client sends the patches, split per file
--- exactly as it shows them, so the report covers the files it lists.
---
--- Fields of `data`:
---   id     string
---   base   string
---   files  EditReportFile[]
function M.request.generate_edit_report(data)
  return M.build("generate_edit_report", data)
end

--- The Session's last generated edit report, if it has one.
---
--- Fields of `data`:
---   id  string
function M.request.edit_report(data)
  return M.build("edit_report", data)
end

--- None while integration is running; otherwise its final result.
---
--- Fields of `data`:
---   id  string
function M.request.session_integration_result(data)
  return M.build("session_integration_result", data)
end

--- Rebase onto the recorded source branch, retaining the checkout.
---
--- Fields of `data`:
---   id  string
function M.request.rebase_session(data)
  return M.build("rebase_session", data)
end

--- Fork the interaction branch and replay its commits onto another local branch.
---
--- Fields of `data`:
---   id      string
---   target  string
function M.request.retarget_session(data)
  return M.build("retarget_session", data)
end

--- Previous branches, oldest first. Their Git refs are retained.
---
--- Fields of `data`:
---   id  string
function M.request.session_branch_history(data)
  return M.build("session_branch_history", data)
end

--- Return the worktree to a branch saved by retargeting, without rebasing it.
---
--- Fields of `data`:
---   id      string
---   branch  string
function M.request.restore_session_branch(data)
  return M.build("restore_session_branch", data)
end

--- Fields of `data`:
---   id  string
function M.request.session_rebase_status(data)
  return M.build("session_rebase_status", data)
end

function M.request.health()
  return M.build("health")
end

--- Fields of `data`:
---   host_path       path
---   name            string|null  (optional)
---   git_repository  path|null  (optional)
function M.request.create_workspace(data)
  return M.build("create_workspace", data)
end

function M.request.list_workspaces()
  return M.build("list_workspaces")
end

--- Deleted Workspaces are available only for listing and restoration.
function M.request.list_deleted_workspaces()
  return M.build("list_deleted_workspaces")
end

--- Soft delete: preserve metadata, Sessions, and checkouts on disk.
---
--- Fields of `data`:
---   id  string
function M.request.delete_workspace(data)
  return M.build("delete_workspace", data)
end

--- Fields of `data`:
---   id  string
function M.request.restore_workspace(data)
  return M.build("restore_workspace", data)
end

--- Fields of `data`:
---   id  string
function M.request.workspace(data)
  return M.build("workspace", data)
end

--- Name the Workspace a host directory belongs to. The path need not be a
--- Workspace root: a directory anywhere beneath one answers with that
--- Workspace, so a client holding only a working directory — an editor,
--- a script run from a subdirectory — can find the Workspace over it
--- without listing them all and comparing paths itself. The innermost
--- Workspace wins when they nest, and the most recently accessed one when
--- several name the same directory. Answers with nothing rather than an
--- error when no Workspace covers the path, which is an ordinary answer.
---
--- Fields of `data`:
---   path  path
function M.request.workspace_for_path(data)
  return M.build("workspace_for_path", data)
end

--- Fields of `data`:
---   id    string
---   name  string|null
function M.request.rename_workspace(data)
  return M.build("rename_workspace", data)
end

--- Associate (or disassociate) a Workspace with a Git checkout. The path
--- may be anywhere inside the checkout; the server stores its root.
---
--- Fields of `data`:
---   workspace_id    string
---   git_repository  path|null
function M.request.set_workspace_git_repository(data)
  return M.build("set_workspace_git_repository", data)
end

--- Point a Workspace at a different host directory. The path must be
--- absolute and must exist; the server stores it canonicalized. The
--- Workspace keeps its id, Sessions and policy; Interactions already
--- running stay where they were spawned.
---
--- Fields of `data`:
---   workspace_id  string
---   host_path     path
function M.request.set_workspace_host_path(data)
  return M.build("set_workspace_host_path", data)
end

--- Read the server-owned Workspace launch policy without touching the
--- Workspace's last-accessed timestamp. Used as a lightweight change feed
--- by clients displaying the Driva options view.
---
--- Fields of `data`:
---   workspace_id  string
function M.request.workspace_launch(data)
  return M.build("workspace_launch", data)
end

--- Fields of `data`:
---   workspace_id     string
---   selection        Selection
---   launch           LaunchPolicy  (optional)
---   create_worktree  boolean  (optional)
---   checkout_from    string|null  (optional)
---   message          string|null  (optional)
---   name             string|null  (optional)
---   contract         Contract|null  (optional)
---   focus            boolean  (optional)
function M.request.create_session(data)
  return M.build("create_session", data)
end

--- Report the Driva policy a `CreateSession` with these inputs would run
--- under. Creates nothing and touches no session state.
---
--- Fields of `data`:
---   workspace_id     string
---   selection        Selection
---   launch           LaunchPolicy  (optional)
---   create_worktree  boolean  (optional)
---   checkout_from    string|null  (optional)
function M.request.plan_session(data)
  return M.build("plan_session", data)
end

--- Name the Driva templates a session in this Workspace could be launched
--- with: Driva's built-ins, overridden by any `driva.toml` the Workspace
--- carries. Resolves the same set `templates` on a launch request is
--- looked up in, so a client can offer exactly what would be accepted.
---
--- Fields of `data`:
---   workspace_id  string
function M.request.list_templates(data)
  return M.build("list_templates", data)
end

--- Models a session can launch on, recently selected first, then catalog
--- order, each with the reasoning-effort rungs it accepts.
function M.request.list_models()
  return M.build("list_models")
end

--- Remember an explicit picker selection in the shared model history.
---
--- Fields of `data`:
---   selection  Selection
function M.request.remember_model(data)
  return M.build("remember_model", data)
end

--- Fields of `data`:
---   id         string
---   launch     LaunchPolicy  (optional)
---   selection  Selection|null  (optional)
function M.request.resume_session(data)
  return M.build("resume_session", data)
end

--- Create and associate a linked Git worktree for an existing Session.
--- Refused when it already has one; the association is used on its next
--- launch or resume.
---
--- Fields of `data`:
---   id  string
function M.request.create_session_worktree(data)
  return M.build("create_session_worktree", data)
end

--- Remove the linked Git worktrees of Sessions the operator has finished
--- with and whose checkouts have nothing uncommitted in them, and record
--- each such Session as working on its branch alone.
---
--- The branch is never touched: the work an agent committed stays exactly
--- where the operator expects to find it in `git branch`, and resuming the
--- Session checks that branch out again. What goes is the directory, which
--- after a completed Session is a copy of the repository per conversation
--- and nothing else.
---
--- Scoped to one Workspace, or to every one when no id is given. Answers
--- with a `CleanedWorktree` per Session considered, including those left
--- alone and why.
---
--- Fields of `data`:
---   workspace_id  string|null  (optional)
function M.request.clean_worktrees(data)
  return M.build("clean_worktrees", data)
end

--- List the linked Git worktrees Styra knows of, each with the Sessions
--- that record it. Changes nothing: it is what an operator asks before
--- deciding what `Request::CleanWorktrees` should take, or to find which
--- conversation a directory under their worktree parent came from.
---
--- Covers every checkout a Session records, including a branch whose
--- directory was cleaned up, and every directory in the Workspace's
--- worktree parent that no Session records. Scoped to one Workspace, or to
--- every one when no id is given.
---
--- Fields of `data`:
---   workspace_id  string|null  (optional)
function M.request.list_worktrees(data)
  return M.build("list_worktrees", data)
end

--- Switch a stopped Session's native transcript to the other interactive
--- provider, retaining its id, checkout, and journal. Records the provider
--- change in the conversation log; the Session can then be resumed.
---
--- Fields of `data`:
---   id  string
function M.request.convert_session_provider(data)
  return M.build("convert_session_provider", data)
end

--- Branch a stored Session's native provider transcript into a new
--- sibling Session in the same Workspace. `history` chooses a prefix
--- through `at_ms` or only the entry at that point; an absent cutoff keeps
--- the whole history and is valid only for the prefix choice. The provider
--- may also change. The source Session, native transcript, and Styra
--- journal are left untouched.
---
--- Fields of `data`:
---   id         string
---   at_ms      number|null  (optional)
---   history    BranchHistory  (optional)
---   provider   Provider|null  (optional)
---   selection  Selection|null  (optional)
function M.request.branch_session(data)
  return M.build("branch_session", data)
end

--- Fields of `data`:
---   id    string
---   name  string|null
function M.request.rename_session(data)
  return M.build("rename_session", data)
end

--- Replace a Session's tags. Tags live with the durable Session, so they
--- remain available after its live interaction stops or resumes.
---
--- Fields of `data`:
---   id    string
---   tags  string[]
function M.request.set_session_tags(data)
  return M.build("set_session_tags", data)
end

--- All tags known to the server, alphabetically.
function M.request.list_tags()
  return M.build("list_tags")
end

--- Read a Session's message box (see `ComposerState`). A Session nobody
--- has typed in yet answers with an empty one.
---
--- Fields of `data`:
---   id  string
function M.request.session_composer(data)
  return M.build("session_composer", data)
end

--- Replace a Session's stored message box.
---
--- Fields of `data`:
---   id        string
---   composer  ComposerState
function M.request.set_session_composer(data)
  return M.build("set_session_composer", data)
end

--- Transcribe one host audio file and return its text.
---
--- The server runs a local Whisper model in its own process. No agent
--- provider, session, or sandbox is involved, so the operation names no
--- selection and costs no interactive quota.
---
--- Fields of `data`:
---   path  path
function M.request.transcribe_audio(data)
  return M.build("transcribe_audio", data)
end

--- Tell the server that its client has begun capturing microphone audio.
--- Recording happens on the client's host; this notification exists so
--- the server's stdout/log still carries the complete transcription
--- lifecycle.
function M.request.audio_recording_started()
  return M.build("audio_recording_started")
end

--- Tell the server that microphone capture has stopped and transcription
--- is about to be requested.
function M.request.audio_recording_stopped()
  return M.build("audio_recording_stopped")
end

--- Report a client-side recording failure to the server's stdout/log.
--- Transcription failures are logged by the server directly.
---
--- Fields of `data`:
---   error  string
function M.request.audio_transcription_error(data)
  return M.build("audio_transcription_error", data)
end

--- Apply one edit to the latest stored Workspace sandbox policy. Applies to
--- launches made after it, not to interactions already running under the
--- old one.
---
--- Fields of `data`:
---   workspace_id  string
---   change        WorkspaceLaunchChange
function M.request.change_workspace_launch(data)
  return M.build("change_workspace_launch", data)
end

--- Fields of `data`:
---   id       string
---   message  SendMessage
function M.request.send_message(data)
  return M.build("send_message", data)
end

--- Switch a live interaction onto another model, applied now and recorded
--- with the session so reopening it keeps the switch. To change provider,
--- stop the interaction and resume it with the new selection instead.
---
--- Fields of `data`:
---   id         string
---   selection  Selection
function M.request.set_session_selection(data)
  return M.build("set_session_selection", data)
end

--- Change the directory used by later turns of a live interaction. The
--- path is on the host and must stay inside the interaction's Workspace.
---
--- Fields of `data`:
---   id         string
---   directory  path
function M.request.set_interaction_working_directory(data)
  return M.build("set_interaction_working_directory", data)
end

--- Keep at it after a rate limit, or stop doing so: when a plan window
--- refuses this interaction's work, resume the Session and ask it again
--- once the window turns over.
---
--- The setting belongs to the interaction the operator is looking at —
--- they are answering for this conversation, not for the account — and is
--- stored with its Session, since the retry itself replaces the
--- interaction it applied to.
---
--- Fields of `data`:
---   id       string
---   enabled  boolean
function M.request.set_interaction_auto_retry(data)
  return M.build("set_interaction_auto_retry", data)
end

--- Commit the work this interaction leaves in its checkout each time it
--- goes idle, or stop doing so. Until the operator answers, it is on for a
--- Session working in a linked checkout of its own and off for one working
--- in the Workspace directory; the answer is stored with the Session, so
--- resuming it keeps it.
---
--- The commit is subject-lined with the start of the turn's last agent
--- message, and its body holds the message that started the turn and
--- every agent message in it.
---
--- Fields of `data`:
---   id       string
---   enabled  boolean
function M.request.set_interaction_auto_commit(data)
  return M.build("set_interaction_auto_commit", data)
end

--- Persist an operator message in the session's durable input queue
--- without sending it yet, so it survives the client disconnecting before
--- the interaction is idle enough to accept it.
---
--- Fields of `data`:
---   id       string
---   message  SendMessage
function M.request.queue_message(data)
  return M.build("queue_message", data)
end

--- Send and remove the oldest durably queued message, if any.
---
--- Fields of `data`:
---   id  string
function M.request.send_queued_message(data)
  return M.build("send_queued_message", data)
end

--- Discard the session's durably queued messages.
---
--- Fields of `data`:
---   id  string
function M.request.clear_queued_messages(data)
  return M.build("clear_queued_messages", data)
end

--- Fields of `data`:
---   id  string
function M.request.interrupt_interaction(data)
  return M.build("interrupt_interaction", data)
end

--- Fields of `data`:
---   id  string
function M.request.stop_interaction(data)
  return M.build("stop_interaction", data)
end

--- Set whether the operator is finished with a Session. Anything but
--- `CompletionState::Active` stops any live interaction serving it (the
--- row stays listed, but clients normally hide completed rows). The state
--- lives with the Session, not the interaction, so it survives the
--- interaction stopping and is what the stored-sessions picker filters
--- on; resuming the Session clears it back to `Active`. Sealing is the
--- exception in both directions: nothing sets a sealed Session back to
--- `Active`, and a sealed Session cannot be resumed.
---
--- Fields of `data`:
---   id         string
---   completed  CompletionState
function M.request.set_session_completed(data)
  return M.build("set_session_completed", data)
end

--- Stop an interaction and drop the server's record of it, so the Session
--- is only what is stored on disk: it no longer appears in the
--- current-interactions list and can be resumed like any other history.
---
--- Fields of `data`:
---   id  string
function M.request.close_interaction(data)
  return M.build("close_interaction", data)
end

--- Fields of `data`:
---   id  string
function M.request.load_interaction(data)
  return M.build("load_interaction", data)
end

--- Fields of `data`:
---   id     string
---   after  number
---   raw    boolean  (optional)
function M.request.updates(data)
  return M.build("updates", data)
end

--- The interactions the server lists — its own and those a previous run
--- left open — that the query admits, newest first. A newtype over an
--- `Option` rather than a struct variant so that a request with no `data`
--- at all, which is what this was before it took a query, still parses,
--- and lists everything.
--- Takes one InteractionQuery|null value, or none at all.
function M.request.list_interactions(value)
  return M.build("list_interactions", value)
end

--- Fields of `data`:
---   workspace_id  string
function M.request.list_sessions(data)
  return M.build("list_sessions", data)
end

--- Fields of `data`:
---   id   string
---   raw  boolean  (optional)
function M.request.stored_session(data)
  return M.build("stored_session", data)
end

--- Read the provider-native, resumable session JSONL for this Session.
---
--- Fields of `data`:
---   id  string
function M.request.provider_raw(data)
  return M.build("provider_raw", data)
end

--- Fields of `data`:
---   id  string
function M.request.shell(data)
  return M.build("shell", data)
end

--- Start a project command in the live session's sandbox, replacing the
--- same action's previous run if it is still going.
---
--- Fields of `data`:
---   id       string
---   action   ProjectAction
---   command  string|null  (optional)
function M.request.start_project_action(data)
  return M.build("start_project_action", data)
end

--- Parse the session's most recent agent message under the contract its
--- last typed turn was sent with, and return the typed value.
---
--- Separate from sending, rather than a reply to it, because a turn takes
--- minutes: the client polls `Request::Updates` as it would for any
--- session and asks for the answer once the turn completes. It reads the
--- same journal the interface renders, so it works on a live interaction
--- and a stored session alike — an answer can be re-parsed long after the
--- interaction it came from has ended.
---
--- Fields of `data`:
---   id        string
---   contract  Contract|null  (optional)
function M.request.turn_answer(data)
  return M.build("turn_answer", data)
end

--- Read the server's in-memory log of plan-quota readings seen on any
--- interaction's wire, oldest first. Server-wide because quota belongs to
--- the account rather than to one session, and in-memory because it is a
--- live reading rather than a record: it starts empty with the daemon.
function M.request.quota_log()
  return M.build("quota_log")
end

--- Read this interaction's durable action history, including earlier runs.
---
--- Fields of `data`:
---   id  string
function M.request.interaction_actions(data)
  return M.build("interaction_actions", data)
end

--- List the processes running in this interaction's sandbox right now:
--- every process in the sandbox's PID namespace, the agent and whatever it
--- started, including daemons that left their parent. A snapshot, not a
--- stream; clients showing it ask again to keep it current.
---
--- Fields of `data`:
---   id  string
function M.request.interaction_processes(data)
  return M.build("interaction_processes", data)
end

--- Read the Driva policy a listed interaction was launched under: what its
--- agent runs, and what it can see and write. Asked of one interaction
--- when a client shows it, rather than carried on every listed row.
---
--- Fields of `data`:
---   id  string
function M.request.interaction_driva(data)
  return M.build("interaction_driva", data)
end

--- What checkout diff mode compares the working tree with: where the
--- interaction's branch would be integrated — the merge base of its
--- checkout and the branch it targets, which a rebase moves on — or, for
--- a checkout with no target branch, its first snapshot rebuilt as a Git
--- tree, uncommitted work at the time included.
---
--- Fields of `data`:
---   id  string
function M.request.diff_base(data)
  return M.build("diff_base", data)
end

--- What one operator turn changed in the checkout, read from the
--- snapshots taken as it started and as it ended — everything, whoever
--- wrote it, rather than what the agent reported writing.
---
--- Every interaction working in a Git checkout has an initial snapshot
--- recorded before the provider starts, then as each turn ends and before
--- each operator message is sent. A snapshot names
--- the commit checked out and, when `git status` reports anything, carries
--- the diff from that commit to the working tree, untracked files
--- included. The turn-end snapshot is taken after any auto-commit of the
--- turn; the one before a message is skipped when the checkout is exactly
--- as the last snapshot recorded it. Snapshots are appended to the
--- Session's durable directory and outlive the interaction.
---
--- A turn that has not ended yet — the latest one, still running — is
--- answered from the most recent snapshot to the checkout as it is now,
--- so asking again shows the turn's progress.
---
--- `turn` counts the Session's operator messages from 1, as a client
--- listing them counts them; for a branch that includes the inherited
--- ones. Answered for a stored Session as for a live one.
---
--- Fields of `data`:
---   id    string
---   turn  number
function M.request.snapshot_turn_diff(data)
  return M.build("snapshot_turn_diff", data)
end

--- Ask the server to remove its socket and exit. Any live interactions it owns die
--- with it, so this is the deliberate counterpart to the daemon outliving
--- its clients.
function M.request.shutdown()
  return M.build("shutdown")
end

return M
