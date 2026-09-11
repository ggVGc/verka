-- The Styra client/server wire vocabulary, as a Lua module.
--
-- Generated from the Serde type definitions in `styra-protocol`; every
-- operation, field name, and enum spelling here is read out of the Rust that
-- defines the protocol, so this file cannot describe a protocol the server
-- does not speak. Do not edit it by hand.
--
--   cargo run -p styra-protocol --bin styra-protocol-lua
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
    { name = "health", payload = { kind = "unit" } },
    { name = "create_workspace", payload = { kind = "newtype", type = { kind = "ref", name = "CreateWorkspace" } } },
    { name = "list_workspaces", payload = { kind = "unit" } },
    { name = "workspace", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
      },
    } },
    { name = "set_workspace_git_repository", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
        { name = "git_repository", required = true, type = { kind = "optional", inner = { kind = "string", path = true } } },
      },
    } },
    { name = "set_workspace_worktrees_enabled", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "workspace_id", required = true, type = { kind = "string" } },
        { name = "enabled", required = true, type = { kind = "boolean" } },
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
    { name = "resume_session", payload = { kind = "newtype", type = { kind = "ref", name = "ResumeSession" } } },
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
      },
    } },
    { name = "rename_session", payload = { kind = "newtype", type = { kind = "ref", name = "RenameSession" } } },
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
    { name = "list_interactions", payload = { kind = "unit" } },
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
    { name = "turn_answer", payload = {
      kind = "struct",
      deny_unknown_fields = true,
      fields = {
        { name = "id", required = true, type = { kind = "string" } },
        { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
      },
    } },
    { name = "quota_log", payload = { kind = "unit" } },
    { name = "shutdown", payload = { kind = "unit" } },
  },
}

--- Versioned request envelope. Flattening keeps `operation` at the top level.
--- Successful response payload. The variant must match the request operation.
M.types.Response = {
  kind = "enum",
  tagging = { style = "adjacent", tag = "type", content = "data" },
  variants = {
    { name = "health", payload = { kind = "newtype", type = { kind = "ref", name = "Health" } } },
    { name = "workspace_created", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspaces", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "WorkspaceSummary" } } } },
    { name = "workspace", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_git_repository_updated", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_worktrees_updated", payload = { kind = "newtype", type = { kind = "ref", name = "WorkspaceSummary" } } },
    { name = "workspace_launch", payload = { kind = "newtype", type = { kind = "ref", name = "LaunchPolicy" } } },
    { name = "session_created", payload = { kind = "newtype", type = { kind = "ref", name = "SessionInfo" } } },
    { name = "session_plan", payload = { kind = "newtype", type = { kind = "ref", name = "DrivaOptions" } } },
    { name = "templates", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "TemplateSummary" } } } },
    { name = "session_resumed", payload = { kind = "newtype", type = { kind = "ref", name = "SessionInfo" } } },
    { name = "session_converted", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
    { name = "session_branched", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
    { name = "session_renamed", payload = { kind = "newtype", type = { kind = "ref", name = "SessionSummary" } } },
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
    { name = "shell", payload = { kind = "newtype", type = { kind = "ref", name = "ShellInfo" } } },
    { name = "answer", payload = { kind = "newtype", type = { kind = "ref", name = "Answer" } } },
    { name = "quota_log", payload = { kind = "newtype", type = { kind = "list", item = { kind = "ref", name = "QuotaEvent" } } } },
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

M.types.CreateWorkspace = {
  kind = "struct",
  fields = {
    { name = "host_path", required = true, type = { kind = "string", path = true } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "git_repository", required = false, type = { kind = "optional", inner = { kind = "string", path = true } } },
  },
}

M.types.CreateSession = {
  kind = "struct",
  deny_unknown_fields = true,
  fields = {
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "launch", required = false, type = { kind = "ref", name = "LaunchPolicy" } },
    { name = "message", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "contract", required = false, type = { kind = "optional", inner = { kind = "ref", name = "Contract" } } },
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

--- Which coding agent a session launches, and thus which command line and wire
--- protocol it gets. The model and reasoning effort are chosen separately (see
--- `Selection`); a provider is only the agent itself.
M.types.Provider = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "codex", payload = { kind = "unit" } },
    { name = "codex-exec", payload = { kind = "unit" } },
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
--- model and effort ran, and re-parsing it reproduces the launch. Parsing accepts
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
    { name = "worktrees_enabled", required = false, type = { kind = "boolean" } },
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

--- A stored session, enough to display and select it from a list — see
--- `crate::journal::list_sessions`.
M.types.SessionSummary = {
  kind = "struct",
  fields = {
    { name = "id", required = true, type = { kind = "string" } },
    { name = "name", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "path", required = true, type = { kind = "string", path = true } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "age", required = true, type = { kind = "string" } },
    { name = "created_at_ms", required = true, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "last_event_at_ms", required = false, type = { kind = "optional", inner = { kind = "number", integer = true } } },
    { name = "last_event_age", required = false, type = { kind = "string" } },
    { name = "origin", required = false, type = { kind = "optional", inner = { kind = "ref", name = "SessionOrigin" } } },
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
    { name = "workspace_id", required = true, type = { kind = "string" } },
    { name = "selection", required = true, type = { kind = "ref", name = "Selection" } },
    { name = "workspace", required = true, type = { kind = "string", path = true } },
    { name = "driva", required = true, type = { kind = "ref", name = "DrivaOptions" } },
    { name = "accepting", required = true, type = { kind = "boolean" } },
    { name = "activity", required = false, type = { kind = "ref", name = "InteractionActivity" } },
    { name = "idle_unseen", required = false, type = { kind = "boolean" } },
    { name = "last_message", required = false, type = { kind = "optional", inner = { kind = "string" } } },
    { name = "auto_retry", required = false, type = { kind = "boolean" } },
    { name = "events", required = false, type = { kind = "number", integer = true } },
  },
}

M.types.StoredSession = {
  kind = "struct",
  fields = {
    { name = "summary", required = true, type = { kind = "ref", name = "SessionSummary" } },
    { name = "events", required = true, type = { kind = "list", item = { kind = "ref", name = "AgentEvent" } } },
    { name = "raw", required = true, type = { kind = "list", item = { kind = "ref", name = "RawLine" } } },
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

--- How much reasoning the model is asked to spend per turn.
---
--- One vocabulary across providers, since the ladders coincide in the middle;
--- `Provider::efforts` narrows it to what a given agent accepts. Passed to
--- codex as its `model_reasoning_effort` config override and to Claude Code as
--- `--effort`.
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

M.types.SequencedUpdate = {
  kind = "struct",
  fields = {
    { name = "sequence", required = true, type = { kind = "number", integer = true } },
    { name = "update", required = true, type = { kind = "ref", name = "InteractionUpdate" } },
  },
}

--- What a live interaction is currently waiting on.
M.types.InteractionActivity = {
  kind = "enum",
  tagging = { style = "external" },
  plain = true,
  variants = {
    { name = "pending", payload = { kind = "unit" } },
    { name = "running", payload = { kind = "unit" } },
    { name = "background", payload = { kind = "unit" } },
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
        { name = "usage", required = true, type = { kind = "ref", name = "TokenUsage" } },
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

M.enums.Request = { "health", "create_workspace", "list_workspaces", "workspace", "set_workspace_git_repository", "set_workspace_worktrees_enabled", "workspace_launch", "create_session", "plan_session", "list_templates", "resume_session", "convert_session_provider", "branch_session", "rename_session", "change_workspace_launch", "send_message", "set_session_selection", "set_interaction_working_directory", "set_interaction_auto_retry", "queue_message", "send_queued_message", "clear_queued_messages", "interrupt_interaction", "stop_interaction", "close_interaction", "load_interaction", "updates", "list_interactions", "list_sessions", "stored_session", "provider_raw", "shell", "turn_answer", "quota_log", "shutdown" }
--- Wire spellings of `Request`.
M.Request = {
  HEALTH = "health",
  CREATE_WORKSPACE = "create_workspace",
  LIST_WORKSPACES = "list_workspaces",
  WORKSPACE = "workspace",
  SET_WORKSPACE_GIT_REPOSITORY = "set_workspace_git_repository",
  SET_WORKSPACE_WORKTREES_ENABLED = "set_workspace_worktrees_enabled",
  WORKSPACE_LAUNCH = "workspace_launch",
  CREATE_SESSION = "create_session",
  PLAN_SESSION = "plan_session",
  LIST_TEMPLATES = "list_templates",
  RESUME_SESSION = "resume_session",
  CONVERT_SESSION_PROVIDER = "convert_session_provider",
  BRANCH_SESSION = "branch_session",
  RENAME_SESSION = "rename_session",
  CHANGE_WORKSPACE_LAUNCH = "change_workspace_launch",
  SEND_MESSAGE = "send_message",
  SET_SESSION_SELECTION = "set_session_selection",
  SET_INTERACTION_WORKING_DIRECTORY = "set_interaction_working_directory",
  SET_INTERACTION_AUTO_RETRY = "set_interaction_auto_retry",
  QUEUE_MESSAGE = "queue_message",
  SEND_QUEUED_MESSAGE = "send_queued_message",
  CLEAR_QUEUED_MESSAGES = "clear_queued_messages",
  INTERRUPT_INTERACTION = "interrupt_interaction",
  STOP_INTERACTION = "stop_interaction",
  CLOSE_INTERACTION = "close_interaction",
  LOAD_INTERACTION = "load_interaction",
  UPDATES = "updates",
  LIST_INTERACTIONS = "list_interactions",
  LIST_SESSIONS = "list_sessions",
  STORED_SESSION = "stored_session",
  PROVIDER_RAW = "provider_raw",
  SHELL = "shell",
  TURN_ANSWER = "turn_answer",
  QUOTA_LOG = "quota_log",
  SHUTDOWN = "shutdown",
}

M.enums.Response = { "health", "workspace_created", "workspaces", "workspace", "workspace_git_repository_updated", "workspace_worktrees_updated", "workspace_launch", "session_created", "session_plan", "templates", "session_resumed", "session_converted", "session_branched", "session_renamed", "workspace_launch_updated", "accepted", "queued", "sent_queued_message", "queued_messages", "interaction_loaded", "updates", "interactions", "stored_sessions", "stored_session", "provider_raw", "shell", "answer", "quota_log" }
--- Wire spellings of `Response`.
M.Response = {
  HEALTH = "health",
  WORKSPACE_CREATED = "workspace_created",
  WORKSPACES = "workspaces",
  WORKSPACE = "workspace",
  WORKSPACE_GIT_REPOSITORY_UPDATED = "workspace_git_repository_updated",
  WORKSPACE_WORKTREES_UPDATED = "workspace_worktrees_updated",
  WORKSPACE_LAUNCH = "workspace_launch",
  SESSION_CREATED = "session_created",
  SESSION_PLAN = "session_plan",
  TEMPLATES = "templates",
  SESSION_RESUMED = "session_resumed",
  SESSION_CONVERTED = "session_converted",
  SESSION_BRANCHED = "session_branched",
  SESSION_RENAMED = "session_renamed",
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
  SHELL = "shell",
  ANSWER = "answer",
  QUOTA_LOG = "quota_log",
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

M.enums.Provider = { "codex", "codex-exec", "claude" }
--- Wire spellings of `Provider`.
M.Provider = {
  CODEX = "codex",
  CODEX_EXEC = "codex-exec",
  CLAUDE = "claude",
}

M.enums.WorkspaceLaunchChange = { "set_network", "set_writable_workspace", "set_templates", "add_mounts", "remove_mount", "replace" }
--- Wire spellings of `WorkspaceLaunchChange`.
M.WorkspaceLaunchChange = {
  SET_NETWORK = "set_network",
  SET_WRITABLE_WORKSPACE = "set_writable_workspace",
  SET_TEMPLATES = "set_templates",
  ADD_MOUNTS = "add_mounts",
  REMOVE_MOUNT = "remove_mount",
  REPLACE = "replace",
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

M.enums.InteractionActivity = { "pending", "running", "background" }
--- Wire spellings of `InteractionActivity`.
M.InteractionActivity = {
  PENDING = "pending",
  RUNNING = "running",
  BACKGROUND = "background",
}

M.enums.AgentEvent = { "user_message", "thread_started", "turn_started", "turn_completed", "usage_updated", "command_started", "command_completed", "file_changed", "diff_updated", "tool_started", "tool_completed", "plan_updated", "agent_message", "thinking", "error", "model_changed", "branched", "task_started", "task_progress", "task_completed", "background_tasks", "unknown", "malformed" }
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

function check_payload(variant, content, path, deny_unknown)
  local payload = variant.payload
  if payload.kind == "unit" then
    if content ~= nil then
      return fail(path, "%q carries no data", variant.name)
    end
    return true
  end
  if content == nil then
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

--- Fields of `data`:
---   id  string
function M.request.workspace(data)
  return M.build("workspace", data)
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

--- Opt in or out of exposing linked-worktree creation to launches in this
--- Workspace.
---
--- Fields of `data`:
---   workspace_id  string
---   enabled       boolean
function M.request.set_workspace_worktrees_enabled(data)
  return M.build("set_workspace_worktrees_enabled", data)
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
---   workspace_id  string
---   selection     Selection
---   launch        LaunchPolicy  (optional)
---   message       string|null  (optional)
---   name          string|null  (optional)
---   contract      Contract|null  (optional)
function M.request.create_session(data)
  return M.build("create_session", data)
end

--- Report the Driva policy a `CreateSession` with these inputs would run
--- under. Creates nothing and touches no session state.
---
--- Fields of `data`:
---   workspace_id  string
---   selection     Selection
---   launch        LaunchPolicy  (optional)
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

--- Fields of `data`:
---   id         string
---   launch     LaunchPolicy  (optional)
---   selection  Selection|null  (optional)
function M.request.resume_session(data)
  return M.build("resume_session", data)
end

--- Convert a stored Session's native provider transcript (Codex rollout or
--- Claude project JSONL) to the other interactive provider's format,
--- using Genta's session conversion. The source Session and its native
--- transcript are left untouched; the result is a new sibling Session in
--- the same Workspace, ready to resume under the other provider. Sugar
--- for `Request::BranchSession` with `at_ms: None` and the other
--- provider named.
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
---   id        string
---   at_ms     number|null  (optional)
---   history   BranchHistory  (optional)
---   provider  Provider|null  (optional)
function M.request.branch_session(data)
  return M.build("branch_session", data)
end

--- Fields of `data`:
---   id    string
---   name  string|null
function M.request.rename_session(data)
  return M.build("rename_session", data)
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
--- with the session so reopening it keeps the switch. The provider cannot
--- change; that needs a new session.
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

function M.request.list_interactions()
  return M.build("list_interactions")
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

--- Ask the server to remove its socket and exit. Any live interactions it owns die
--- with it, so this is the deliberate counterpart to the daemon outliving
--- its clients.
function M.request.shutdown()
  return M.build("shutdown")
end

return M
