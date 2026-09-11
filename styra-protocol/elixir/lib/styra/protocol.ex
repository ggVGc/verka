# The Styra client/server wire vocabulary, as an Elixir module.
#
# Generated from the Serde type definitions in `styra-protocol`; every
# operation, field name, and enum spelling here is read out of the Rust that
# defines the protocol, so this file cannot describe a protocol the server
# does not speak. Do not edit it by hand.
#
#   cargo run -p styra-protocol --bin styra-codegen -- elixir
#
# It carries no transport and no JSON codec, exactly as the Rust crate does
# not: a request is a plain map for your own encoder to serialise and your
# own socket to carry, one JSON object per line.
#
#   {:ok, request} = Styra.Protocol.Request.send_message(%{
#     id: session,
#     message: %{text: "hello"}
#   })
#   {:ok, answer} = Styra.Protocol.expect(reply, Styra.Protocol.Response.answer())
#

defmodule Styra.Protocol do
  @moduledoc ~S"""
  The Styra client/server wire vocabulary.
 
  Generated from the Serde type definitions in `styra-protocol`, so every
  operation, field name, and enum spelling is the one the server speaks.
 
  A request is a plain map; carrying it is the caller's business — one JSON
  object per line over the server's Unix socket. `Styra.Protocol.Request`
  builds them, `validate/2` checks any wire type, and `unwrap/1` and
  `expect/2` read what comes back.
  """


  # Typed answers
  # -------------

  @doc "The opening delimiter a contract's answer block sits in."
  def answer_open, do: "<styra:answer>"

  @doc "The closing delimiter a contract's answer block sits in."
  def answer_close, do: "</styra:answer>"

  # Wire types
  # ----------

  @types %{
    # One JSON request sent as a single line over the Unix socket.
    "Request" => %{
      kind: :enum,
      tagging: %{style: :adjacent, tag: "operation", content: "data"},
      deny_unknown_fields: true,
      variants: [
        %{name: "health", payload: %{kind: :unit}},
        %{name: "create_workspace", payload: %{kind: :newtype, type: %{kind: :ref, name: "CreateWorkspace"}}},
        %{name: "list_workspaces", payload: %{kind: :unit}},
        %{name: "workspace", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "set_workspace_git_repository", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "workspace_id", required: true, type: %{kind: :string}},
            %{name: "git_repository", required: true, type: %{kind: :optional, inner: %{kind: :string, path: true}}}
          ]
        }},
        %{name: "set_workspace_worktrees_enabled", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "workspace_id", required: true, type: %{kind: :string}},
            %{name: "enabled", required: true, type: %{kind: :boolean}}
          ]
        }},
        %{name: "workspace_launch", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "workspace_id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "create_session", payload: %{kind: :newtype, type: %{kind: :ref, name: "CreateSession"}}},
        %{name: "plan_session", payload: %{kind: :newtype, type: %{kind: :ref, name: "PlanSession"}}},
        %{name: "list_templates", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "workspace_id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "resume_session", payload: %{kind: :newtype, type: %{kind: :ref, name: "ResumeSession"}}},
        %{name: "convert_session_provider", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "branch_session", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "at_ms", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
            %{name: "history", required: false, type: %{kind: :ref, name: "BranchHistory"}},
            %{name: "provider", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Provider"}}}
          ]
        }},
        %{name: "rename_session", payload: %{kind: :newtype, type: %{kind: :ref, name: "RenameSession"}}},
        %{name: "change_workspace_launch", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "workspace_id", required: true, type: %{kind: :string}},
            %{name: "change", required: true, type: %{kind: :ref, name: "WorkspaceLaunchChange"}}
          ]
        }},
        %{name: "send_message", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "message", required: true, type: %{kind: :ref, name: "SendMessage"}}
          ]
        }},
        %{name: "set_session_selection", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "selection", required: true, type: %{kind: :ref, name: "Selection"}}
          ]
        }},
        %{name: "set_interaction_working_directory", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "directory", required: true, type: %{kind: :string, path: true}}
          ]
        }},
        %{name: "set_interaction_auto_retry", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "enabled", required: true, type: %{kind: :boolean}}
          ]
        }},
        %{name: "queue_message", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "message", required: true, type: %{kind: :ref, name: "SendMessage"}}
          ]
        }},
        %{name: "send_queued_message", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "clear_queued_messages", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "interrupt_interaction", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "stop_interaction", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "close_interaction", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "load_interaction", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "updates", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "after", required: true, type: %{kind: :number, integer: true}},
            %{name: "raw", required: false, type: %{kind: :boolean}}
          ]
        }},
        %{name: "list_interactions", payload: %{kind: :unit}},
        %{name: "list_sessions", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "workspace_id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "stored_session", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "raw", required: false, type: %{kind: :boolean}}
          ]
        }},
        %{name: "provider_raw", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "shell", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "turn_answer", payload: %{
          kind: :struct,
          deny_unknown_fields: true,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "contract", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Contract"}}}
          ]
        }},
        %{name: "quota_log", payload: %{kind: :unit}},
        %{name: "shutdown", payload: %{kind: :unit}}
      ]
    },

    # Versioned request envelope. Flattening keeps `operation` at the top level.
    # Successful response payload. The variant must match the request operation.
    "Response" => %{
      kind: :enum,
      tagging: %{style: :adjacent, tag: "type", content: "data"},
      variants: [
        %{name: "health", payload: %{kind: :newtype, type: %{kind: :ref, name: "Health"}}},
        %{name: "workspace_created", payload: %{kind: :newtype, type: %{kind: :ref, name: "WorkspaceSummary"}}},
        %{name: "workspaces", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "WorkspaceSummary"}}}},
        %{name: "workspace", payload: %{kind: :newtype, type: %{kind: :ref, name: "WorkspaceSummary"}}},
        %{name: "workspace_git_repository_updated", payload: %{kind: :newtype, type: %{kind: :ref, name: "WorkspaceSummary"}}},
        %{name: "workspace_worktrees_updated", payload: %{kind: :newtype, type: %{kind: :ref, name: "WorkspaceSummary"}}},
        %{name: "workspace_launch", payload: %{kind: :newtype, type: %{kind: :ref, name: "LaunchPolicy"}}},
        %{name: "session_created", payload: %{kind: :newtype, type: %{kind: :ref, name: "SessionInfo"}}},
        %{name: "session_plan", payload: %{kind: :newtype, type: %{kind: :ref, name: "DrivaOptions"}}},
        %{name: "templates", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "TemplateSummary"}}}},
        %{name: "session_resumed", payload: %{kind: :newtype, type: %{kind: :ref, name: "SessionInfo"}}},
        %{name: "session_converted", payload: %{kind: :newtype, type: %{kind: :ref, name: "SessionSummary"}}},
        %{name: "session_branched", payload: %{kind: :newtype, type: %{kind: :ref, name: "SessionSummary"}}},
        %{name: "session_renamed", payload: %{kind: :newtype, type: %{kind: :ref, name: "SessionSummary"}}},
        %{name: "workspace_launch_updated", payload: %{kind: :newtype, type: %{kind: :ref, name: "LaunchPolicy"}}},
        %{name: "accepted", payload: %{kind: :unit}},
        %{name: "queued", payload: %{kind: :newtype, type: %{kind: :number, integer: true}}},
        %{name: "sent_queued_message", payload: %{kind: :tuple, items: [%{kind: :optional, inner: %{kind: :ref, name: "QueuedMessage"}}, %{kind: :list, item: %{kind: :ref, name: "QueuedMessage"}}]}},
        %{name: "queued_messages", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "QueuedMessage"}}}},
        %{name: "interaction_loaded", payload: %{kind: :newtype, type: %{kind: :ref, name: "LoadedInteraction"}}},
        %{name: "updates", payload: %{kind: :newtype, type: %{kind: :ref, name: "Updates"}}},
        %{name: "interactions", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "InteractionSummary"}}}},
        %{name: "stored_sessions", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "SessionSummary"}}}},
        %{name: "stored_session", payload: %{kind: :newtype, type: %{kind: :ref, name: "StoredSession"}}},
        %{name: "provider_raw", payload: %{kind: :newtype, type: %{kind: :ref, name: "ProviderRaw"}}},
        %{name: "shell", payload: %{kind: :newtype, type: %{kind: :ref, name: "ShellInfo"}}},
        %{name: "answer", payload: %{kind: :newtype, type: %{kind: :ref, name: "Answer"}}},
        %{name: "quota_log", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "QuotaEvent"}}}}
      ]
    },

    # Response envelope returned for every syntactically valid connection.
    "WireResponse" => %{
      kind: :enum,
      tagging: %{style: :internal, tag: "status"},
      variants: [
        %{name: "ok", payload: %{
          kind: :struct,
          fields: [
            %{name: "response", required: true, type: %{kind: :ref, name: "Response"}}
          ]
        }},
        %{name: "error", payload: %{
          kind: :struct,
          fields: [
            %{name: "error", required: true, type: %{kind: :string}}
          ]
        }}
      ]
    },

    "CreateWorkspace" => %{
      kind: :struct,
      fields: [
        %{name: "host_path", required: true, type: %{kind: :string, path: true}},
        %{name: "name", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "git_repository", required: false, type: %{kind: :optional, inner: %{kind: :string, path: true}}}
      ]
    },

    "CreateSession" => %{
      kind: :struct,
      deny_unknown_fields: true,
      fields: [
        %{name: "workspace_id", required: true, type: %{kind: :string}},
        %{name: "selection", required: true, type: %{kind: :ref, name: "Selection"}},
        %{name: "launch", required: false, type: %{kind: :ref, name: "LaunchPolicy"}},
        %{name: "message", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "name", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "contract", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Contract"}}}
      ]
    },

    # Ask what a new session in this Workspace *would* be launched under, without
    # creating one. Carries exactly the launch inputs of `CreateSession` that
    # shape the sandbox, so the answer is the policy that session would get.
    "PlanSession" => %{
      kind: :struct,
      deny_unknown_fields: true,
      fields: [
        %{name: "workspace_id", required: true, type: %{kind: :string}},
        %{name: "selection", required: true, type: %{kind: :ref, name: "Selection"}},
        %{name: "launch", required: false, type: %{kind: :ref, name: "LaunchPolicy"}}
      ]
    },

    "ResumeSession" => %{
      kind: :struct,
      deny_unknown_fields: true,
      fields: [
        %{name: "id", required: true, type: %{kind: :string}},
        %{name: "launch", required: false, type: %{kind: :ref, name: "LaunchPolicy"}},
        %{name: "selection", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Selection"}}}
      ]
    },

    # The source history used to seed a branch.
    "BranchHistory" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "through_selected", payload: %{kind: :unit}},
        %{name: "selected_only", payload: %{kind: :unit}}
      ]
    },

    # Which coding agent a session launches, and thus which command line and wire
    # protocol it gets. The model and reasoning effort are chosen separately (see
    # `Selection`); a provider is only the agent itself.
    "Provider" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "codex", payload: %{kind: :unit}},
        %{name: "codex-exec", payload: %{kind: :unit}},
        %{name: "claude", payload: %{kind: :unit}}
      ]
    },

    "RenameSession" => %{
      kind: :struct,
      deny_unknown_fields: true,
      fields: [
        %{name: "id", required: true, type: %{kind: :string}},
        %{name: "name", required: true, type: %{kind: :optional, inner: %{kind: :string}}}
      ]
    },

    # One server-owned edit to a Workspace's standing launch policy.
    #
    # Clients send intent instead of replacing a locally cached copy. This lets
    # the server apply the edit to the latest stored policy and return the
    # authoritative result, avoiding lost updates between multiple Styra UIs.
    "WorkspaceLaunchChange" => %{
      kind: :enum,
      tagging: %{style: :adjacent, tag: "change", content: "value"},
      variants: [
        %{name: "set_network", payload: %{kind: :newtype, type: %{kind: :optional, inner: %{kind: :boolean}}}},
        %{name: "set_writable_workspace", payload: %{kind: :newtype, type: %{kind: :optional, inner: %{kind: :boolean}}}},
        %{name: "set_templates", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :string}}}},
        %{name: "add_mounts", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "LaunchMount"}}}},
        %{name: "remove_mount", payload: %{kind: :newtype, type: %{kind: :ref, name: "LaunchMount"}}},
        %{name: "replace", payload: %{kind: :newtype, type: %{kind: :ref, name: "LaunchPolicy"}}}
      ]
    },

    "SendMessage" => %{
      kind: :struct,
      fields: [
        %{name: "text", required: true, type: %{kind: :string}},
        %{name: "selection", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Selection"}}},
        %{name: "contract", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Contract"}}}
      ]
    },

    # What an operator picked to launch: an agent, a model, and a reasoning effort.
    #
    # All three are always present. A selection never leaves the model or effort to
    # whatever the agent happens to be configured for, because that configuration is
    # invisible to Genta and to anything reading a journal afterwards — a session
    # recorded as plain `codex` says nothing about what actually ran. A profile name
    # that omits either therefore takes this provider's declared default
    # (`Provider::default_model`, `Provider::default_effort`) rather than
    # standing for "unset".
    #
    # A selection round-trips through one string, `Selection::name`, of the form
    # `provider:model/effort` — `codex:gpt-5.6-terra/medium`,
    # `claude:claude-opus-5/xhigh`. That string is the profile name, so it is also
    # what a journal records and a status line shows: a stored session states which
    # model and effort ran, and re-parsing it reproduces the launch. Parsing accepts
    # the shorter `provider[:model][/effort]` forms and fills in the defaults, so
    # `--profile claude` still works and names itself fully afterwards.
    "Selection" => %{
      kind: :struct,
      fields: [
        %{name: "provider", required: true, type: %{kind: :ref, name: "Provider"}},
        %{name: "model", required: true, type: %{kind: :string}},
        %{name: "effort", required: true, type: %{kind: :ref, name: "Effort"}}
      ]
    },

    # The shape a client asks a turn's answer to come back in.
    #
    # A contract is applied at both ends of one turn: it frames the message sent
    # to the agent with instructions describing the shape, and it parses the
    # agent's reply back into `AnswerValue`. Framing server-side is what keeps
    # clients honest — every caller asks for a shape the same way, so the parser
    # only has to understand one phrasing. See `crate::contract`.
    "Contract" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "text", payload: %{kind: :unit}},
        %{name: "lines", payload: %{kind: :unit}},
        %{name: "files", payload: %{kind: :unit}},
        %{name: "json", payload: %{kind: :unit}}
      ]
    },

    "Health" => %{
      kind: :struct,
      fields: [
        %{name: "service", required: true, type: %{kind: :string}}
      ]
    },

    # A durable Styra Workspace, which groups provider Sessions that operate on
    # the same host directory.
    "WorkspaceSummary" => %{
      kind: :struct,
      fields: [
        %{name: "id", required: true, type: %{kind: :string}},
        %{name: "name", required: true, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "host_path", required: true, type: %{kind: :string, path: true}},
        %{name: "git_repository", required: false, type: %{kind: :optional, inner: %{kind: :string, path: true}}},
        %{name: "worktrees_enabled", required: false, type: %{kind: :boolean}},
        %{name: "path", required: true, type: %{kind: :string, path: true}},
        %{name: "session_count", required: true, type: %{kind: :number, integer: true}},
        %{name: "age", required: true, type: %{kind: :string}},
        %{name: "created_at_ms", required: true, type: %{kind: :number, integer: true}},
        %{name: "last_accessed_at_ms", required: false, type: %{kind: :number, integer: true}},
        %{name: "launch", required: false, type: %{kind: :ref, name: "LaunchPolicy"}}
      ]
    },

    # The sandbox policy inputs a launch asks for beyond the agent selection.
    #
    # The same type serves two roles, and the difference is only where it is
    # stored. A Workspace holds one as its *standing* policy — what every launch
    # there starts from (`WorkspaceSummary::launch`). A launch request carries
    # one as its own *overlay* — what this interaction adds to, or says instead
    # of, the Workspace's. `LaunchPolicy::merge` is the only place the two are
    # combined, and the server merges them for `create_session`, `plan_session`
    # and `resume_session` alike, so a plan cannot disagree with the launch it
    # describes.
    #
    # Nothing here is resolved: `templates` are names and `mounts` are requests.
    # The server resolves both against the Workspace's `driva.toml` and the host
    # filesystem after merging.
    "LaunchPolicy" => %{
      kind: :struct,
      deny_unknown_fields: true,
      fields: [
        %{name: "network", required: false, type: %{kind: :optional, inner: %{kind: :boolean}}},
        %{name: "writable_workspace", required: false, type: %{kind: :optional, inner: %{kind: :boolean}}},
        %{name: "templates", required: false, type: %{kind: :list, item: %{kind: :string}}},
        %{name: "mounts", required: false, type: %{kind: :list, item: %{kind: :ref, name: "LaunchMount"}}},
        %{name: "ignore_workspace", required: false, type: %{kind: :boolean}}
      ]
    },

    "SessionInfo" => %{
      kind: :struct,
      fields: [
        %{name: "id", required: true, type: %{kind: :string}},
        %{name: "name", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "workspace_id", required: true, type: %{kind: :string}},
        %{name: "selection", required: true, type: %{kind: :ref, name: "Selection"}},
        %{name: "workspace", required: true, type: %{kind: :string, path: true}},
        %{name: "journal_path", required: true, type: %{kind: :string, path: true}},
        %{name: "driva", required: true, type: %{kind: :ref, name: "DrivaOptions"}},
        %{name: "updates_after", required: false, type: %{kind: :number, integer: true}},
        %{name: "queued", required: false, type: %{kind: :list, item: %{kind: :ref, name: "QueuedMessage"}}}
      ]
    },

    # A human-facing summary of the Driva policy an interaction was launched with:
    # the isolation backend, the command it runs, and the mount/network policy
    # enforced around it. Captured once at spawn time from the same
    # `ExecutionRequest` Driva itself executes (see `DrivaOptions::capture` in
    # `crate::interaction`), so it can never drift from what is actually running.
    "DrivaOptions" => %{
      kind: :struct,
      fields: [
        %{name: "isolation_backend", required: true, type: %{kind: :string}},
        %{name: "command", required: true, type: %{kind: :list, item: %{kind: :string}}},
        %{name: "working_directory", required: true, type: %{kind: :string, path: true}},
        %{name: "network", required: true, type: %{kind: :boolean}},
        %{name: "mounts", required: true, type: %{kind: :list, item: %{kind: :ref, name: "AttributedMount"}}},
        %{name: "base", required: false, type: %{kind: :list, item: %{kind: :ref, name: "BaseCapability"}}}
      ]
    },

    # A Driva execution template the server can offer, named and described, so a
    # client can present the real set rather than asking the operator to recall
    # template names.
    "TemplateSummary" => %{
      kind: :struct,
      fields: [
        %{name: "name", required: true, type: %{kind: :string}},
        %{name: "description", required: false, type: %{kind: :string}}
      ]
    },

    # A stored session, enough to display and select it from a list — see
    # `crate::journal::list_sessions`.
    "SessionSummary" => %{
      kind: :struct,
      fields: [
        %{name: "id", required: true, type: %{kind: :string}},
        %{name: "name", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "workspace_id", required: true, type: %{kind: :string}},
        %{name: "path", required: true, type: %{kind: :string, path: true}},
        %{name: "selection", required: true, type: %{kind: :ref, name: "Selection"}},
        %{name: "age", required: true, type: %{kind: :string}},
        %{name: "created_at_ms", required: true, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "last_event_at_ms", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "last_event_age", required: false, type: %{kind: :string}},
        %{name: "origin", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "SessionOrigin"}}}
      ]
    },

    # An operator message persisted but not yet sent.
    #
    # Carries the contract it was queued under, so a message the operator asked
    # for a shape while the agent was busy still asks for it when the queue
    # drains. Without that the shape would be dropped at exactly the moment the
    # operator could least do anything about it.
    "QueuedMessage" => %{
      kind: :struct,
      fields: [
        %{name: "text", required: true, type: %{kind: :string}},
        %{name: "contract", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "Contract"}}}
      ]
    },

    # Everything a client needs to make one live interaction current, returned
    # by one ordinary blocking request.
    "LoadedInteraction" => %{
      kind: :struct,
      fields: [
        %{name: "summary", required: true, type: %{kind: :ref, name: "InteractionSummary"}},
        %{name: "updates", required: true, type: %{kind: :ref, name: "Updates"}},
        %{name: "queued", required: true, type: %{kind: :list, item: %{kind: :ref, name: "QueuedMessage"}}}
      ]
    },

    "Updates" => %{
      kind: :struct,
      fields: [
        %{name: "updates", required: true, type: %{kind: :list, item: %{kind: :ref, name: "SequencedUpdate"}}},
        %{name: "next", required: true, type: %{kind: :number, integer: true}}
      ]
    },

    # An interaction the server is currently running (this process's live sessions),
    # enough to list it and to reattach a client to it. Distinct from
    # `SessionSummary`, which describes a session persisted in the store
    # whether or not it is still live.
    "InteractionSummary" => %{
      kind: :struct,
      fields: [
        %{name: "id", required: true, type: %{kind: :string}},
        %{name: "name", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "workspace_id", required: true, type: %{kind: :string}},
        %{name: "selection", required: true, type: %{kind: :ref, name: "Selection"}},
        %{name: "workspace", required: true, type: %{kind: :string, path: true}},
        %{name: "driva", required: true, type: %{kind: :ref, name: "DrivaOptions"}},
        %{name: "accepting", required: true, type: %{kind: :boolean}},
        %{name: "activity", required: false, type: %{kind: :ref, name: "InteractionActivity"}},
        %{name: "idle_unseen", required: false, type: %{kind: :boolean}},
        %{name: "last_message", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "auto_retry", required: false, type: %{kind: :boolean}},
        %{name: "events", required: false, type: %{kind: :number, integer: true}}
      ]
    },

    "StoredSession" => %{
      kind: :struct,
      fields: [
        %{name: "summary", required: true, type: %{kind: :ref, name: "SessionSummary"}},
        %{name: "events", required: true, type: %{kind: :list, item: %{kind: :ref, name: "AgentEvent"}}},
        %{name: "raw", required: true, type: %{kind: :list, item: %{kind: :ref, name: "RawLine"}}}
      ]
    },

    # A provider's own persisted session file, kept distinct from Styra's wire
    # journal. The text is verbatim JSONL, including records that never appeared
    # on the app-server wire.
    "ProviderRaw" => %{
      kind: :struct,
      fields: [
        %{name: "provider", required: true, type: %{kind: :ref, name: "Provider"}},
        %{name: "text", required: true, type: %{kind: :string}}
      ]
    },

    # Host-side tmux endpoint for the shell owned by a live session's sandbox.
    "ShellInfo" => %{
      kind: :struct,
      fields: [
        %{name: "tmux", required: true, type: %{kind: :string, path: true}},
        %{name: "socket", required: true, type: %{kind: :string, path: true}}
      ]
    },

    # One turn's typed answer.
    #
    # A reply that did not satisfy its contract is an `Answer` too, not an error
    # in place of one: `value` is absent, `error` says what was wrong, and
    # `source` still carries what the agent actually said. An agent that answered
    # well but framed it badly has produced something worth reading, and a client
    # that was handed only "no answer block" could not show it.
    "Answer" => %{
      kind: :struct,
      fields: [
        %{name: "contract", required: true, type: %{kind: :ref, name: "Contract"}},
        %{name: "value", required: false, type: %{kind: :optional, inner: %{kind: :ref, name: "AnswerValue"}}},
        %{name: "error", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
        %{name: "source", required: true, type: %{kind: :string}}
      ]
    },

    # One reading of a plan quota window, as a provider reported it mid-session.
    #
    # Both interactive providers volunteer these unprompted, in different shapes
    # and with different amounts of detail — see `crate::quota`, which reads
    # them off the wire and keeps them.
    "QuotaEvent" => %{
      kind: :struct,
      fields: [
        %{name: "at_ms", required: true, type: %{kind: :number, integer: true}},
        %{name: "session_id", required: true, type: %{kind: :string}},
        %{name: "provider", required: true, type: %{kind: :ref, name: "Provider"}},
        %{name: "window", required: true, type: %{kind: :string}},
        %{name: "status", required: true, type: %{kind: :ref, name: "QuotaStatus"}},
        %{name: "utilization", required: false, type: %{kind: :optional, inner: %{kind: :number}}},
        %{name: "resets_at_ms", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "detail", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
      ]
    },

    # One extra host directory the operator asked to be bound into the sandbox,
    # on top of what the profile and the selected templates already grant.
    #
    # This is a *request*, not a resolved mount: the source is whatever the
    # operator typed, and the server canonicalizes it (rejecting a path that does
    # not exist) before it becomes part of a launch. An absent `destination`
    # means "the same path inside the sandbox", matching Driva's own rule for a
    # bind mount with no destination.
    "LaunchMount" => %{
      kind: :struct,
      deny_unknown_fields: true,
      fields: [
        %{name: "source", required: true, type: %{kind: :string, path: true}},
        %{name: "destination", required: false, type: %{kind: :optional, inner: %{kind: :string, path: true}}},
        %{name: "writable", required: false, type: %{kind: :boolean}}
      ]
    },

    # How much reasoning the model is asked to spend per turn.
    #
    # One vocabulary across providers, since the ladders coincide in the middle;
    # `Provider::efforts` narrows it to what a given agent accepts. Passed to
    # codex as its `model_reasoning_effort` config override and to Claude Code as
    # `--effort`.
    "Effort" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "minimal", payload: %{kind: :unit}},
        %{name: "low", payload: %{kind: :unit}},
        %{name: "medium", payload: %{kind: :unit}},
        %{name: "high", payload: %{kind: :unit}},
        %{name: "xhigh", payload: %{kind: :unit}},
        %{name: "max", payload: %{kind: :unit}}
      ]
    },

    # A resolved mount together with the layer that asked for it.
    "AttributedMount" => %{
      kind: :struct,
      fields: [
        %{name: "origin", required: true, type: %{kind: :ref, name: "MountOrigin"}},
        %{name: "mount", required: true, type: %{kind: :ref, name: "Mount"}}
      ]
    },

    # One capability of the sandbox's base system, as it resolved on this host.
    #
    # A capability is the portable statement ("this sandbox can resolve host
    # names"); the paths are what that means on this machine. Reporting both lets
    # an operator see not only what the private root holds but why it holds it.
    "BaseCapability" => %{
      kind: :struct,
      fields: [
        %{name: "name", required: true, type: %{kind: :string}},
        %{name: "description", required: true, type: %{kind: :string}},
        %{name: "entries", required: true, type: %{kind: :list, item: %{kind: :ref, name: "BaseEntry"}}},
        %{name: "environment", required: false, type: %{kind: :list, item: %{kind: :string}}}
      ]
    },

    # Where a Session came from, when it was not launched fresh but branched
    # from another one — see `crate::server::ServerState::branch_session`.
    # Recorded once, at branch time; it never updates as the source Session
    # keeps being worked on afterwards, the same way a git branch's fork point
    # does not move when the source gets new commits.
    "SessionOrigin" => %{
      kind: :struct,
      fields: [
        %{name: "session_id", required: true, type: %{kind: :string}},
        %{name: "provider", required: true, type: %{kind: :ref, name: "Provider"}},
        %{name: "at_ms", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "history", required: false, type: %{kind: :ref, name: "BranchHistory"}}
      ]
    },

    "SequencedUpdate" => %{
      kind: :struct,
      fields: [
        %{name: "sequence", required: true, type: %{kind: :number, integer: true}},
        %{name: "update", required: true, type: %{kind: :ref, name: "InteractionUpdate"}}
      ]
    },

    # What a live interaction is currently waiting on.
    "InteractionActivity" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "pending", payload: %{kind: :unit}},
        %{name: "running", payload: %{kind: :unit}},
        %{name: "background", payload: %{kind: :unit}}
      ]
    },

    # The stable, provider-independent event vocabulary.
    "AgentEvent" => %{
      kind: :enum,
      tagging: %{style: :internal, tag: "type"},
      variants: [
        %{name: "user_message", payload: %{
          kind: :struct,
          fields: [
            %{name: "text", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "thread_started", payload: %{
          kind: :struct,
          fields: [
            %{name: "thread_id", required: true, type: %{kind: :string}},
            %{name: "model", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
            %{name: "effort", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
          ]
        }},
        %{name: "turn_started", payload: %{kind: :unit}},
        %{name: "turn_completed", payload: %{
          kind: :struct,
          fields: [
            %{name: "usage", required: true, type: %{kind: :ref, name: "TokenUsage"}}
          ]
        }},
        %{name: "usage_updated", payload: %{
          kind: :struct,
          fields: [
            %{name: "usage", required: true, type: %{kind: :ref, name: "TokenUsage"}}
          ]
        }},
        %{name: "command_started", payload: %{
          kind: :struct,
          fields: [
            %{name: "command", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "command_completed", payload: %{
          kind: :struct,
          fields: [
            %{name: "command", required: true, type: %{kind: :string}},
            %{name: "status", required: true, type: %{kind: :string}},
            %{name: "exit_code", required: true, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
            %{name: "output", required: false, type: %{kind: :string}}
          ]
        }},
        %{name: "file_changed", payload: %{
          kind: :struct,
          fields: [
            %{name: "id", required: false, type: %{kind: :string}},
            %{name: "paths", required: true, type: %{kind: :list, item: %{kind: :string}}},
            %{name: "diff", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
            %{name: "checkpoint", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
            %{name: "checkpoint_error", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
          ]
        }},
        %{name: "diff_updated", payload: %{
          kind: :struct,
          fields: [
            %{name: "diff", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "tool_started", payload: %{
          kind: :struct,
          fields: [
            %{name: "id", required: false, type: %{kind: :string}},
            %{name: "name", required: true, type: %{kind: :string}},
            %{name: "detail", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "tool_completed", payload: %{
          kind: :struct,
          fields: [
            %{name: "id", required: false, type: %{kind: :string}},
            %{name: "name", required: true, type: %{kind: :string}},
            %{name: "detail", required: false, type: %{kind: :string}},
            %{name: "status", required: true, type: %{kind: :string}},
            %{name: "output", required: false, type: %{kind: :string}}
          ]
        }},
        %{name: "plan_updated", payload: %{
          kind: :struct,
          fields: [
            %{name: "text", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "agent_message", payload: %{
          kind: :struct,
          fields: [
            %{name: "text", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "thinking", payload: %{
          kind: :struct,
          fields: [
            %{name: "text", required: true, type: %{kind: :string}},
            %{name: "tokens", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}}
          ]
        }},
        %{name: "error", payload: %{
          kind: :struct,
          fields: [
            %{name: "message", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "model_changed", payload: %{
          kind: :struct,
          fields: [
            %{name: "model", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
            %{name: "effort", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
          ]
        }},
        %{name: "branched", payload: %{
          kind: :struct,
          fields: [
            %{name: "direction", required: true, type: %{kind: :ref, name: "BranchDirection"}},
            %{name: "session", required: true, type: %{kind: :string}},
            %{name: "name", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
          ]
        }},
        %{name: "task_started", payload: %{
          kind: :struct,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "description", required: true, type: %{kind: :string}},
            %{name: "kind", required: true, type: %{kind: :string}},
            %{name: "agent", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
          ]
        }},
        %{name: "task_progress", payload: %{
          kind: :struct,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "description", required: true, type: %{kind: :string}},
            %{name: "agent", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
            %{name: "tool", required: false, type: %{kind: :optional, inner: %{kind: :string}}},
            %{name: "tokens", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}}
          ]
        }},
        %{name: "task_completed", payload: %{
          kind: :struct,
          fields: [
            %{name: "id", required: true, type: %{kind: :string}},
            %{name: "status", required: true, type: %{kind: :string}},
            %{name: "summary", required: false, type: %{kind: :string}},
            %{name: "error", required: false, type: %{kind: :optional, inner: %{kind: :string}}}
          ]
        }},
        %{name: "background_tasks", payload: %{
          kind: :struct,
          fields: [
            %{name: "running", required: true, type: %{kind: :number, integer: true}}
          ]
        }},
        %{name: "unknown", payload: %{
          kind: :struct,
          fields: [
            %{name: "wire_type", required: true, type: %{kind: :string}}
          ]
        }},
        %{name: "malformed", payload: %{
          kind: :struct,
          fields: [
            %{name: "error", required: true, type: %{kind: :string}}
          ]
        }}
      ]
    },

    # One verbatim line of the agent interaction, undecoded.
    "RawLine" => %{
      kind: :struct,
      fields: [
        %{name: "direction", required: true, type: %{kind: :ref, name: "Direction"}},
        %{name: "text", required: true, type: %{kind: :string}},
        %{name: "at_ms", required: false, type: %{kind: :number, integer: true}}
      ]
    },

    # A parsed answer. The variant always matches the `Contract` that produced
    # it, so a client can dispatch on the value alone.
    "AnswerValue" => %{
      kind: :enum,
      tagging: %{style: :adjacent, tag: "contract", content: "value"},
      variants: [
        %{name: "text", payload: %{kind: :newtype, type: %{kind: :string}}},
        %{name: "lines", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :string}}}},
        %{name: "files", payload: %{kind: :newtype, type: %{kind: :list, item: %{kind: :ref, name: "FileLocation"}}}},
        %{name: "json", payload: %{kind: :newtype, type: %{kind: :any}}}
      ]
    },

    # How much of a quota window is left, as the provider judges it.
    "QuotaStatus" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "allowed", payload: %{kind: :unit}},
        %{name: "warning", payload: %{kind: :unit}},
        %{name: "exhausted", payload: %{kind: :unit}}
      ]
    },

    # Which layer of the launch policy put a mount in the sandbox.
    #
    # The effective policy is one flat list by the time Driva runs it, but the
    # layers that produced it are not interchangeable to an operator: a grant from
    # the agent profile is a property of the agent they picked, one from a
    # template is a property of the template, and one they typed is theirs to take
    # back. Recording the layer at the point the list is built is the only place
    # the answer is known for certain — afterwards a mount is just a path pair.
    "MountOrigin" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "workspace", payload: %{kind: :unit}},
        %{name: "git_repository", payload: %{kind: :unit}},
        %{name: "scratch", payload: %{kind: :unit}},
        %{name: "profile", payload: %{kind: :unit}},
        %{name: "template", payload: %{kind: :unit}},
        %{name: "tooling", payload: %{kind: :unit}},
        %{name: "operator", payload: %{kind: :unit}},
        %{name: "broker", payload: %{kind: :unit}}
      ]
    },

    # A filesystem made available inside an isolated execution.
    "Mount" => %{
      kind: :enum,
      tagging: %{style: :internal, tag: "kind"},
      variants: [
        %{name: "bind", payload: %{
          kind: :struct,
          fields: [
            %{name: "source", required: true, type: %{kind: :string, path: true}},
            %{name: "destination", required: true, type: %{kind: :string, path: true}},
            %{name: "access", required: true, type: %{kind: :ref, name: "MountAccess"}}
          ]
        }},
        %{name: "temporary", payload: %{
          kind: :struct,
          fields: [
            %{name: "destination", required: true, type: %{kind: :string, path: true}}
          ]
        }},
        %{name: "overlay", payload: %{
          kind: :struct,
          fields: [
            %{name: "source", required: true, type: %{kind: :string, path: true}},
            %{name: "destination", required: true, type: %{kind: :string, path: true}}
          ]
        }}
      ]
    },

    # One path the base lays down, and where its content comes from when that is
    # not the same place (a host link followed out of the base).
    "BaseEntry" => %{
      kind: :struct,
      fields: [
        %{name: "path", required: true, type: %{kind: :string, path: true}},
        %{name: "source", required: false, type: %{kind: :optional, inner: %{kind: :string, path: true}}}
      ]
    },

    # An update delivered from the interaction's threads to the UI.
    "InteractionUpdate" => %{
      kind: :enum,
      tagging: %{style: :adjacent, tag: "type", content: "data"},
      variants: [
        %{name: "event", payload: %{kind: :newtype, type: %{kind: :ref, name: "AgentEvent"}}},
        %{name: "raw", payload: %{kind: :newtype, type: %{kind: :ref, name: "RawLine"}}},
        %{name: "log", payload: %{kind: :newtype, type: %{kind: :ref, name: "LogEntry"}}},
        %{name: "quota", payload: %{kind: :newtype, type: %{kind: :ref, name: "QuotaEvent"}}},
        %{name: "working_directory_changed", payload: %{kind: :newtype, type: %{kind: :string, path: true}}},
        %{name: "ended", payload: %{kind: :newtype, type: %{kind: :ref, name: "InteractionEnd"}}}
      ]
    },

    "TokenUsage" => %{
      kind: :struct,
      fields: [
        %{name: "input_tokens", required: false, type: %{kind: :number, integer: true}},
        %{name: "cached_input_tokens", required: false, type: %{kind: :number, integer: true}},
        %{name: "output_tokens", required: false, type: %{kind: :number, integer: true}},
        %{name: "reasoning_output_tokens", required: false, type: %{kind: :number, integer: true}}
      ]
    },

    # Which side of a branch an `AgentEvent::Branched` marker sits on. Each
    # marker names the other session, so the direction says which way the link
    # points rather than which session it belongs to.
    "BranchDirection" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "from", payload: %{kind: :unit}},
        %{name: "to", payload: %{kind: :unit}}
      ]
    },

    # Which way a wire line travelled.
    "Direction" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "to_agent", payload: %{kind: :unit}},
        %{name: "from_agent", payload: %{kind: :unit}}
      ]
    },

    # A place in the Workspace, as named by a `Contract::Files` answer.
    #
    # `line` and `column` are 1-based and `None` when the agent named none, which
    # is the difference between "this file" and "this position", and is why they
    # are not zero-defaulted into a position that does not exist.
    "FileLocation" => %{
      kind: :struct,
      fields: [
        %{name: "path", required: true, type: %{kind: :string, path: true}},
        %{name: "line", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "column", required: false, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "description", required: false, type: %{kind: :string}}
      ]
    },

    "MountAccess" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "readonly", payload: %{kind: :unit}},
        %{name: "readwrite", payload: %{kind: :unit}}
      ]
    },

    # One line in the log view: a Styra-internal note or a line of agent stderr.
    "LogEntry" => %{
      kind: :struct,
      fields: [
        %{name: "level", required: true, type: %{kind: :ref, name: "LogLevel"}},
        %{name: "message", required: true, type: %{kind: :string}}
      ]
    },

    # How an interaction finished.
    "InteractionEnd" => %{
      kind: :struct,
      fields: [
        %{name: "exit_code", required: true, type: %{kind: :optional, inner: %{kind: :number, integer: true}}},
        %{name: "error", required: true, type: %{kind: :optional, inner: %{kind: :string}}}
      ]
    },

    # Severity of a `LogEntry`, used to colour the log view.
    "LogLevel" => %{
      kind: :enum,
      tagging: %{style: :external},
      plain: true,
      variants: [
        %{name: "info", payload: %{kind: :unit}},
        %{name: "warn", payload: %{kind: :unit}},
        %{name: "error", payload: %{kind: :unit}}
      ]
    }
  }

  @doc ~S"""
  Every type that crosses the socket, described well enough to check a value
  against it. See `validate/2`.
  """
  @spec types() :: %{String.t() => map()}
  def types, do: @types

  @doc "The descriptor for one wire type, or nil."
  @spec type(String.t()) :: map() | nil
  def type(name), do: Map.get(@types, name)

  # Operations
  # ----------

  @operations [
    "health",
    "create_workspace",
    "list_workspaces",
    "workspace",
    "set_workspace_git_repository",
    "set_workspace_worktrees_enabled",
    "workspace_launch",
    "create_session",
    "plan_session",
    "list_templates",
    "resume_session",
    "convert_session_provider",
    "branch_session",
    "rename_session",
    "change_workspace_launch",
    "send_message",
    "set_session_selection",
    "set_interaction_working_directory",
    "set_interaction_auto_retry",
    "queue_message",
    "send_queued_message",
    "clear_queued_messages",
    "interrupt_interaction",
    "stop_interaction",
    "close_interaction",
    "load_interaction",
    "updates",
    "list_interactions",
    "list_sessions",
    "stored_session",
    "provider_raw",
    "shell",
    "turn_answer",
    "quota_log",
    "shutdown"
  ]

  @doc "Every operation the server answers, in protocol order."
  @spec operations() :: [String.t()]
  def operations, do: @operations

  # Runtime
  # -------

  # Hand-written support the generated tables are useless without: a validator
  # driven by `types/0`, and the two halves of a request/response exchange.
  # Everything here is generic over the descriptors; it says nothing about any
  # particular operation, so it does not need regenerating when the protocol
  # gains one.
  #
  # Elixir needs no null sentinel, which the Lua library does: a map holds nil
  # as a value and `Map.fetch/2` still tells an absent key from one set to it.
  # So the distinction the protocol cares about — clearing a Session name is
  # not the same as not mentioning it — is said the obvious way, by leaving the
  # key out or setting it to nil.

  @doc ~S"""
  Check a value against a named wire type.

  Returns `:ok`, or `{:error, message}` with a message naming the field that
  was wrong. Works on anything in `types/0`: an `InteractionUpdate` off the
  update stream as readily as a request.

      :ok = Styra.Protocol.validate("LaunchPolicy", %{network: true})
  """
  @spec validate(String.t(), term()) :: :ok | {:error, String.t()}
  def validate(name, value) do
    case Map.fetch(types(), name) do
      :error -> {:error, ~s(unknown wire type "#{name}")}
      {:ok, _descriptor} -> check_value(%{kind: :ref, name: name}, value, "")
    end
  end

  @doc ~S"""
  Build the request for `operation` from `data`, checking it first.

  Returns `{:ok, request}` or `{:error, message}`. The request is a plain map
  with string keys, for your own encoder to serialise and your own socket to
  carry — one JSON object per line.

  The check is worth having because `Request` denies unknown fields: a client
  that sends a misspelled key gets a socket round trip and an error string
  back instead of an answer, far from the line that made the mistake.

      {:ok, request} = Styra.Protocol.build("rename_session", %{id: "styra-1", name: nil})
  """
  @spec build(String.t(), term()) :: {:ok, map()} | {:error, String.t()}
  def build(operation, data \\ :none)

  def build(operation, data) do
    descriptor = Map.fetch!(types(), "Request")

    case variant_named(descriptor, operation) do
      nil ->
        {:error, ~s("#{operation}" is not a Styra operation)}

      %{payload: %{kind: :unit}} ->
        if data == :none do
          {:ok, %{"operation" => operation}}
        else
          {:error, "#{operation} takes no data"}
        end

      variant ->
        cond do
          data == :none ->
            {:error, "#{operation} needs its data"}

          true ->
            case check_payload(variant, {:given, data}, "data", deny?(descriptor)) do
              :ok -> {:ok, %{"operation" => operation, "data" => data}}
              {:error, message} -> {:error, "#{operation}: #{message}"}
            end
        end
    end
  end

  @doc ~S"""
  `build/2`, raising `ArgumentError` on a request the server would refuse.
  """
  @spec build!(String.t(), term()) :: map()
  def build!(operation, data \\ :none) do
    case build(operation, data) do
      {:ok, request} -> request
      {:error, message} -> raise ArgumentError, message
    end
  end

  @doc ~S"""
  Read a decoded `WireResponse`: `{:ok, response}` carrying the response map
  (its `"type"` and `"data"`), or `{:error, message}` with the server's own
  error.

      {:ok, response} = Styra.Protocol.unwrap(Jason.decode!(line))
  """
  @spec unwrap(term()) :: {:ok, term()} | {:error, String.t()}
  def unwrap(wire) when not is_map(wire) do
    {:error, "the server's reply is not an object (#{kind_of(wire)})"}
  end

  def unwrap(wire) do
    case keyed(wire) do
      %{"status" => "ok", "response" => response} ->
        {:ok, response}

      %{"status" => "ok"} ->
        {:error, "the server's reply is ok but carries no response"}

      %{"status" => "error", "error" => message} when is_binary(message) ->
        {:error, message}

      %{"status" => "error"} ->
        {:error, "the server reported an error with no message"}

      %{"status" => status} ->
        {:error, "the server's reply has an unknown status (#{inspect(status)})"}

      _ ->
        {:error, "the server's reply has no status"}
    end
  end

  @doc ~S"""
  The data of a response of the expected type, or `{:error, message}`. Saves
  every caller the same two checks: that the request succeeded, and that the
  reply is about what was asked.

      {:ok, health} = Styra.Protocol.expect(reply, Styra.Protocol.Response.health())
  """
  @spec expect(term(), String.t()) :: {:ok, term()} | {:error, String.t()}
  def expect(reply, kind) do
    with {:ok, response} <- unwrap(reply) do
      case is_map(response) && keyed(response) do
        %{"type" => ^kind} = response -> {:ok, Map.get(response, "data")}
        %{"type" => other} -> {:error, ~s(expected a "#{kind}" response, got "#{other}")}
        _ -> {:error, "the server's response names no type"}
      end
    end
  end

  # Checking ----------------------------------------------------------------

  defp check_value(%{kind: :optional, inner: inner}, value, path) do
    if is_nil(value), do: :ok, else: check_value(inner, value, path)
  end

  defp check_value(_shape, nil, path), do: fail(path, "is not nullable")

  defp check_value(%{kind: :any}, _value, _path), do: :ok

  defp check_value(%{kind: :string}, value, path) do
    if is_binary(value), do: :ok, else: fail(path, "expected a string, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :number} = shape, value, path) do
    cond do
      not is_number(value) ->
        fail(path, "expected a number, got #{kind_of(value)}")

      Map.get(shape, :integer, false) && not is_integer(value) ->
        fail(path, "expected a whole number, got #{inspect(value)}")

      true ->
        :ok
    end
  end

  defp check_value(%{kind: :boolean}, value, path) do
    if is_boolean(value), do: :ok, else: fail(path, "expected a boolean, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :list, item: item}, value, path) when is_list(value) do
    value
    |> Enum.with_index()
    |> reduce_ok(fn {element, index} -> check_value(item, element, path_of(path, index)) end)
  end

  defp check_value(%{kind: :list}, value, path) do
    fail(path, "expected a list, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :map, value: shape}, value, path) when is_map(value) do
    reduce_ok(value, fn {key, item} ->
      if is_binary(key) or is_atom(key) do
        check_value(shape, item, path_of(path, key))
      else
        fail(path, "has a non-string key")
      end
    end)
  end

  defp check_value(%{kind: :map}, value, path) do
    fail(path, "expected a map, got #{kind_of(value)}")
  end

  defp check_value(%{kind: :ref, name: name}, value, path) do
    case Map.fetch(types(), name) do
      :error ->
        fail(path, ~s(refers to unknown wire type "#{name}"))

      {:ok, %{kind: :struct} = descriptor} ->
        check_fields(descriptor.fields, value, path, deny?(descriptor))

      {:ok, descriptor} ->
        check_enum(descriptor, value, path)
    end
  end

  defp check_value(shape, _value, path) do
    fail(path, "has no shape the generator understands (#{inspect(shape)})")
  end

  # `reserved` names keys that belong to the encoding rather than to the fields
  # (an internal tag sitting alongside them), so they do not read as unknown.
  defp check_fields(fields, value, path, deny_unknown, reserved \\ [])

  defp check_fields(_fields, value, path, _deny_unknown, _reserved) when not is_map(value) do
    fail(path, "expected a map, got #{kind_of(value)}")
  end

  defp check_fields(fields, value, path, deny_unknown, reserved) do
    given = keyed(value)

    with :ok <- reduce_ok(fields, &check_field(&1, given, path)) do
      if deny_unknown do
        known = MapSet.new(Enum.map(fields, & &1.name) ++ reserved)

        case Enum.find(Map.keys(given), &(not MapSet.member?(known, &1))) do
          nil -> :ok
          unknown -> fail(path, ~s(unknown field "#{unknown}"))
        end
      else
        :ok
      end
    end
  end

  defp check_field(field, given, path) do
    case Map.fetch(given, field.name) do
      :error ->
        if field.required do
          fail(path, ~s(missing required field "#{field.name}"))
        else
          :ok
        end

      {:ok, value} ->
        check_value(field.type, value, path_of(path, field.name))
    end
  end

  # `content` is `:absent` or `{:given, value}`, because nil is a value here
  # and not an absence.
  defp check_payload(%{payload: %{kind: :unit}} = variant, content, path, _deny_unknown) do
    if content == :absent, do: :ok, else: fail(path, ~s("#{variant.name}" carries no data))
  end

  defp check_payload(variant, :absent, path, _deny_unknown) do
    fail(path, ~s("#{variant.name}" needs its data))
  end

  defp check_payload(%{payload: %{kind: :newtype, type: type}}, {:given, content}, path, _deny) do
    check_value(type, content, path)
  end

  defp check_payload(%{payload: %{kind: :tuple, items: items}}, {:given, content}, path, _deny) do
    if is_list(content) and length(content) == length(items) do
      items
      |> Enum.zip(content)
      |> Enum.with_index()
      |> reduce_ok(fn {{item, value}, index} -> check_value(item, value, path_of(path, index)) end)
    else
      fail(path, "expected a list of #{length(items)} values, got #{kind_of(content)}")
    end
  end

  defp check_payload(%{payload: %{kind: :struct} = payload}, {:given, content}, path, deny) do
    check_fields(payload.fields, content, path, Map.get(payload, :deny_unknown_fields, false) || deny)
  end

  # Dispatched on a value rather than by pattern, deliberately. `@types` is a
  # literal, so the compiler knows exactly which tagging styles this protocol
  # uses and reports a clause for any it does not as dead code — which would
  # make a generic runtime warn on every compile, and warn *differently* as
  # the protocol gains and loses shapes. The styles are a closed set that the
  # generator understands whether or not today's protocol uses all of them.
  defp check_enum(descriptor, value, path) do
    style = descriptor.tagging.style

    cond do
      style == :untagged -> :ok
      Map.get(descriptor, :plain, false) -> check_plain(descriptor, value, path)
      style == :external -> check_external(descriptor, value, path)
      true -> check_by_tag(descriptor, value, path)
    end
  end

  defp check_plain(descriptor, value, path) do
    cond do
      not is_binary(value) ->
        fail(path, "expected one of #{spellings_of(descriptor)}, got #{kind_of(value)}")

      is_nil(variant_named(descriptor, value)) ->
        fail(path, ~s("#{value}" is not one of #{spellings_of(descriptor)}))

      true ->
        :ok
    end
  end

  defp check_external(descriptor, value, path) when is_binary(value) do
    case variant_named(descriptor, value) do
      nil -> fail(path, ~s("#{value}" is not one of #{spellings_of(descriptor)}))
      variant -> check_payload(variant, :absent, path, deny?(descriptor))
    end
  end

  defp check_external(descriptor, value, path) do
    cond do
      not is_map(value) ->
        fail(path, "expected a map or a string, got #{kind_of(value)}")

      map_size(value) == 0 ->
        fail(path, "names no variant; expected one of #{spellings_of(descriptor)}")

      map_size(value) > 1 ->
        fail(path, "names more than one variant")

      true ->
        [{name, content}] = Map.to_list(keyed(value))

        case variant_named(descriptor, name) do
          nil ->
            fail(path, ~s("#{name}" is not one of #{spellings_of(descriptor)}))

          variant ->
            check_payload(variant, {:given, content}, path_of(path, name), deny?(descriptor))
        end
    end
  end

  defp check_by_tag(descriptor, value, path) when is_map(value) do
    given = keyed(value)
    tag = descriptor.tagging.tag

    case Map.fetch(given, tag) do
      {:ok, name} when is_binary(name) ->
        case variant_named(descriptor, name) do
          nil -> fail(path, ~s("#{name}" is not one of #{spellings_of(descriptor)}))
          variant -> check_tagged(descriptor, variant, given, path)
        end

      _ ->
        fail(path, ~s(has no "#{tag}" naming one of #{spellings_of(descriptor)}))
    end
  end

  defp check_by_tag(_descriptor, value, path) do
    fail(path, "expected a map, got #{kind_of(value)}")
  end

  defp check_tagged(descriptor, variant, given, path) do
    if descriptor.tagging.style == :adjacent do
      check_adjacent(descriptor, variant, given, path)
    else
      check_internal(descriptor, variant, given, path)
    end
  end

  defp check_adjacent(descriptor, variant, given, path) do
    tagging = descriptor.tagging

    content =
      case Map.fetch(given, tagging.content) do
        {:ok, value} -> {:given, value}
        :error -> :absent
      end

    with :ok <- check_payload(variant, content, path_of(path, tagging.content), deny?(descriptor)) do
      if deny?(descriptor) do
        extra = Enum.find(Map.keys(given), &(&1 != tagging.tag and &1 != tagging.content))
        if extra, do: fail(path, ~s(unknown field "#{extra}")), else: :ok
      else
        :ok
      end
    end
  end

  # Internally tagged: the payload's fields sit beside the tag.
  defp check_internal(descriptor, variant, given, path) do
    tagging = descriptor.tagging

    case variant.payload do
      %{kind: :newtype, type: type} ->
        check_value(type, given, path)

      %{kind: :struct, fields: fields} ->
        check_fields(fields, given, path, deny?(descriptor), [tagging.tag])

      _ ->
        check_fields([], given, path, deny?(descriptor), [tagging.tag])
    end
  end

  # Odds and ends -----------------------------------------------------------

  defp variant_named(descriptor, name) do
    Enum.find(descriptor.variants, &(&1.name == name))
  end

  defp spellings_of(descriptor) do
    descriptor.variants |> Enum.map(&~s("#{&1.name}")) |> Enum.join(", ")
  end

  defp deny?(descriptor), do: Map.get(descriptor, :deny_unknown_fields, false)

  # A map decoded from JSON has string keys; one written by hand in Elixir
  # usually has atom ones. Both are read, because a client that has to spell
  # its own request in string keys is being made to work for the generator.
  defp keyed(map) do
    Map.new(map, fn {key, value} -> {to_string(key), value} end)
  end

  defp path_of("", key), do: to_string(key)
  defp path_of(path, key), do: path <> "." <> to_string(key)

  defp fail("", message), do: {:error, message}
  defp fail(path, message), do: {:error, path <> ": " <> message}

  defp reduce_ok(items, check) do
    Enum.reduce_while(items, :ok, fn item, :ok ->
      case check.(item) do
        :ok -> {:cont, :ok}
        {:error, _} = error -> {:halt, error}
      end
    end)
  end

  defp kind_of(nil), do: "nil"
  defp kind_of(value) when is_binary(value), do: "a string"
  defp kind_of(value) when is_boolean(value), do: "a boolean"
  defp kind_of(value) when is_number(value), do: "a number"
  defp kind_of(value) when is_list(value), do: "a list"
  defp kind_of(value) when is_map(value), do: "a map"
  defp kind_of(value) when is_atom(value), do: "an atom"
  defp kind_of(value), do: inspect(value)

  # Requests
  # --------

  defmodule Request do
    @moduledoc ~S"""
    One function per operation, each returning `{:ok, request}` or
    `{:error, message}` after checking what it was given. The `!` form
    beside each returns the request and raises instead.
    """

    def health, do: Styra.Protocol.build("health")

    @doc "`health/0`, raising on a request the server would refuse."
    def health!, do: Styra.Protocol.build!("health")

    @doc ~S"""
    Fields of `data`:

      * `host_path     `  path
      * `name          `  string|null  (optional)
      * `git_repository`  path|null  (optional)
    """
    def create_workspace(data), do: Styra.Protocol.build("create_workspace", data)

    @doc "`create_workspace/1`, raising on a request the server would refuse."
    def create_workspace!(data), do: Styra.Protocol.build!("create_workspace", data)

    def list_workspaces, do: Styra.Protocol.build("list_workspaces")

    @doc "`list_workspaces/0`, raising on a request the server would refuse."
    def list_workspaces!, do: Styra.Protocol.build!("list_workspaces")

    @doc ~S"""
    Fields of `data`:

      * `id`  string
    """
    def workspace(data), do: Styra.Protocol.build("workspace", data)

    @doc "`workspace/1`, raising on a request the server would refuse."
    def workspace!(data), do: Styra.Protocol.build!("workspace", data)

    @doc ~S"""
    Associate (or disassociate) a Workspace with a Git checkout. The path
    may be anywhere inside the checkout; the server stores its root.

    Fields of `data`:

      * `workspace_id  `  string
      * `git_repository`  path|null
    """
    def set_workspace_git_repository(data), do: Styra.Protocol.build("set_workspace_git_repository", data)

    @doc "`set_workspace_git_repository/1`, raising on a request the server would refuse."
    def set_workspace_git_repository!(data), do: Styra.Protocol.build!("set_workspace_git_repository", data)

    @doc ~S"""
    Opt in or out of exposing linked-worktree creation to launches in this
    Workspace.

    Fields of `data`:

      * `workspace_id`  string
      * `enabled     `  boolean
    """
    def set_workspace_worktrees_enabled(data), do: Styra.Protocol.build("set_workspace_worktrees_enabled", data)

    @doc "`set_workspace_worktrees_enabled/1`, raising on a request the server would refuse."
    def set_workspace_worktrees_enabled!(data), do: Styra.Protocol.build!("set_workspace_worktrees_enabled", data)

    @doc ~S"""
    Read the server-owned Workspace launch policy without touching the
    Workspace's last-accessed timestamp. Used as a lightweight change feed
    by clients displaying the Driva options view.

    Fields of `data`:

      * `workspace_id`  string
    """
    def workspace_launch(data), do: Styra.Protocol.build("workspace_launch", data)

    @doc "`workspace_launch/1`, raising on a request the server would refuse."
    def workspace_launch!(data), do: Styra.Protocol.build!("workspace_launch", data)

    @doc ~S"""
    Fields of `data`:

      * `workspace_id`  string
      * `selection   `  Selection
      * `launch      `  LaunchPolicy  (optional)
      * `message     `  string|null  (optional)
      * `name        `  string|null  (optional)
      * `contract    `  Contract|null  (optional)
    """
    def create_session(data), do: Styra.Protocol.build("create_session", data)

    @doc "`create_session/1`, raising on a request the server would refuse."
    def create_session!(data), do: Styra.Protocol.build!("create_session", data)

    @doc ~S"""
    Report the Driva policy a `CreateSession` with these inputs would run
    under. Creates nothing and touches no session state.

    Fields of `data`:

      * `workspace_id`  string
      * `selection   `  Selection
      * `launch      `  LaunchPolicy  (optional)
    """
    def plan_session(data), do: Styra.Protocol.build("plan_session", data)

    @doc "`plan_session/1`, raising on a request the server would refuse."
    def plan_session!(data), do: Styra.Protocol.build!("plan_session", data)

    @doc ~S"""
    Name the Driva templates a session in this Workspace could be launched
    with: Driva's built-ins, overridden by any `driva.toml` the Workspace
    carries. Resolves the same set `templates` on a launch request is
    looked up in, so a client can offer exactly what would be accepted.

    Fields of `data`:

      * `workspace_id`  string
    """
    def list_templates(data), do: Styra.Protocol.build("list_templates", data)

    @doc "`list_templates/1`, raising on a request the server would refuse."
    def list_templates!(data), do: Styra.Protocol.build!("list_templates", data)

    @doc ~S"""
    Fields of `data`:

      * `id       `  string
      * `launch   `  LaunchPolicy  (optional)
      * `selection`  Selection|null  (optional)
    """
    def resume_session(data), do: Styra.Protocol.build("resume_session", data)

    @doc "`resume_session/1`, raising on a request the server would refuse."
    def resume_session!(data), do: Styra.Protocol.build!("resume_session", data)

    @doc ~S"""
    Convert a stored Session's native provider transcript (Codex rollout or
    Claude project JSONL) to the other interactive provider's format,
    using Genta's session conversion. The source Session and its native
    transcript are left untouched; the result is a new sibling Session in
    the same Workspace, ready to resume under the other provider. Sugar
    for `Request::BranchSession` with `at_ms: None` and the other
    provider named.

    Fields of `data`:

      * `id`  string
    """
    def convert_session_provider(data), do: Styra.Protocol.build("convert_session_provider", data)

    @doc "`convert_session_provider/1`, raising on a request the server would refuse."
    def convert_session_provider!(data), do: Styra.Protocol.build!("convert_session_provider", data)

    @doc ~S"""
    Branch a stored Session's native provider transcript into a new
    sibling Session in the same Workspace. `history` chooses a prefix
    through `at_ms` or only the entry at that point; an absent cutoff keeps
    the whole history and is valid only for the prefix choice. The provider
    may also change. The source Session, native transcript, and Styra
    journal are left untouched.

    Fields of `data`:

      * `id      `  string
      * `at_ms   `  number|null  (optional)
      * `history `  BranchHistory  (optional)
      * `provider`  Provider|null  (optional)
    """
    def branch_session(data), do: Styra.Protocol.build("branch_session", data)

    @doc "`branch_session/1`, raising on a request the server would refuse."
    def branch_session!(data), do: Styra.Protocol.build!("branch_session", data)

    @doc ~S"""
    Fields of `data`:

      * `id  `  string
      * `name`  string|null
    """
    def rename_session(data), do: Styra.Protocol.build("rename_session", data)

    @doc "`rename_session/1`, raising on a request the server would refuse."
    def rename_session!(data), do: Styra.Protocol.build!("rename_session", data)

    @doc ~S"""
    Apply one edit to the latest stored Workspace sandbox policy. Applies to
    launches made after it, not to interactions already running under the
    old one.

    Fields of `data`:

      * `workspace_id`  string
      * `change      `  WorkspaceLaunchChange
    """
    def change_workspace_launch(data), do: Styra.Protocol.build("change_workspace_launch", data)

    @doc "`change_workspace_launch/1`, raising on a request the server would refuse."
    def change_workspace_launch!(data), do: Styra.Protocol.build!("change_workspace_launch", data)

    @doc ~S"""
    Fields of `data`:

      * `id     `  string
      * `message`  SendMessage
    """
    def send_message(data), do: Styra.Protocol.build("send_message", data)

    @doc "`send_message/1`, raising on a request the server would refuse."
    def send_message!(data), do: Styra.Protocol.build!("send_message", data)

    @doc ~S"""
    Switch a live interaction onto another model, applied now and recorded
    with the session so reopening it keeps the switch. The provider cannot
    change; that needs a new session.

    Fields of `data`:

      * `id       `  string
      * `selection`  Selection
    """
    def set_session_selection(data), do: Styra.Protocol.build("set_session_selection", data)

    @doc "`set_session_selection/1`, raising on a request the server would refuse."
    def set_session_selection!(data), do: Styra.Protocol.build!("set_session_selection", data)

    @doc ~S"""
    Change the directory used by later turns of a live interaction. The
    path is on the host and must stay inside the interaction's Workspace.

    Fields of `data`:

      * `id       `  string
      * `directory`  path
    """
    def set_interaction_working_directory(data), do: Styra.Protocol.build("set_interaction_working_directory", data)

    @doc "`set_interaction_working_directory/1`, raising on a request the server would refuse."
    def set_interaction_working_directory!(data), do: Styra.Protocol.build!("set_interaction_working_directory", data)

    @doc ~S"""
    Keep at it after a rate limit, or stop doing so: when a plan window
    refuses this interaction's work, resume the Session and ask it again
    once the window turns over.

    The setting belongs to the interaction the operator is looking at —
    they are answering for this conversation, not for the account — and is
    stored with its Session, since the retry itself replaces the
    interaction it applied to.

    Fields of `data`:

      * `id     `  string
      * `enabled`  boolean
    """
    def set_interaction_auto_retry(data), do: Styra.Protocol.build("set_interaction_auto_retry", data)

    @doc "`set_interaction_auto_retry/1`, raising on a request the server would refuse."
    def set_interaction_auto_retry!(data), do: Styra.Protocol.build!("set_interaction_auto_retry", data)

    @doc ~S"""
    Persist an operator message in the session's durable input queue
    without sending it yet, so it survives the client disconnecting before
    the interaction is idle enough to accept it.

    Fields of `data`:

      * `id     `  string
      * `message`  SendMessage
    """
    def queue_message(data), do: Styra.Protocol.build("queue_message", data)

    @doc "`queue_message/1`, raising on a request the server would refuse."
    def queue_message!(data), do: Styra.Protocol.build!("queue_message", data)

    @doc ~S"""
    Send and remove the oldest durably queued message, if any.

    Fields of `data`:

      * `id`  string
    """
    def send_queued_message(data), do: Styra.Protocol.build("send_queued_message", data)

    @doc "`send_queued_message/1`, raising on a request the server would refuse."
    def send_queued_message!(data), do: Styra.Protocol.build!("send_queued_message", data)

    @doc ~S"""
    Discard the session's durably queued messages.

    Fields of `data`:

      * `id`  string
    """
    def clear_queued_messages(data), do: Styra.Protocol.build("clear_queued_messages", data)

    @doc "`clear_queued_messages/1`, raising on a request the server would refuse."
    def clear_queued_messages!(data), do: Styra.Protocol.build!("clear_queued_messages", data)

    @doc ~S"""
    Fields of `data`:

      * `id`  string
    """
    def interrupt_interaction(data), do: Styra.Protocol.build("interrupt_interaction", data)

    @doc "`interrupt_interaction/1`, raising on a request the server would refuse."
    def interrupt_interaction!(data), do: Styra.Protocol.build!("interrupt_interaction", data)

    @doc ~S"""
    Fields of `data`:

      * `id`  string
    """
    def stop_interaction(data), do: Styra.Protocol.build("stop_interaction", data)

    @doc "`stop_interaction/1`, raising on a request the server would refuse."
    def stop_interaction!(data), do: Styra.Protocol.build!("stop_interaction", data)

    @doc ~S"""
    Stop an interaction and drop the server's record of it, so the Session
    is only what is stored on disk: it no longer appears in the
    current-interactions list and can be resumed like any other history.

    Fields of `data`:

      * `id`  string
    """
    def close_interaction(data), do: Styra.Protocol.build("close_interaction", data)

    @doc "`close_interaction/1`, raising on a request the server would refuse."
    def close_interaction!(data), do: Styra.Protocol.build!("close_interaction", data)

    @doc ~S"""
    Fields of `data`:

      * `id`  string
    """
    def load_interaction(data), do: Styra.Protocol.build("load_interaction", data)

    @doc "`load_interaction/1`, raising on a request the server would refuse."
    def load_interaction!(data), do: Styra.Protocol.build!("load_interaction", data)

    @doc ~S"""
    Fields of `data`:

      * `id   `  string
      * `after`  number
      * `raw  `  boolean  (optional)
    """
    def updates(data), do: Styra.Protocol.build("updates", data)

    @doc "`updates/1`, raising on a request the server would refuse."
    def updates!(data), do: Styra.Protocol.build!("updates", data)

    def list_interactions, do: Styra.Protocol.build("list_interactions")

    @doc "`list_interactions/0`, raising on a request the server would refuse."
    def list_interactions!, do: Styra.Protocol.build!("list_interactions")

    @doc ~S"""
    Fields of `data`:

      * `workspace_id`  string
    """
    def list_sessions(data), do: Styra.Protocol.build("list_sessions", data)

    @doc "`list_sessions/1`, raising on a request the server would refuse."
    def list_sessions!(data), do: Styra.Protocol.build!("list_sessions", data)

    @doc ~S"""
    Fields of `data`:

      * `id `  string
      * `raw`  boolean  (optional)
    """
    def stored_session(data), do: Styra.Protocol.build("stored_session", data)

    @doc "`stored_session/1`, raising on a request the server would refuse."
    def stored_session!(data), do: Styra.Protocol.build!("stored_session", data)

    @doc ~S"""
    Read the provider-native, resumable session JSONL for this Session.

    Fields of `data`:

      * `id`  string
    """
    def provider_raw(data), do: Styra.Protocol.build("provider_raw", data)

    @doc "`provider_raw/1`, raising on a request the server would refuse."
    def provider_raw!(data), do: Styra.Protocol.build!("provider_raw", data)

    @doc ~S"""
    Fields of `data`:

      * `id`  string
    """
    def shell(data), do: Styra.Protocol.build("shell", data)

    @doc "`shell/1`, raising on a request the server would refuse."
    def shell!(data), do: Styra.Protocol.build!("shell", data)

    @doc ~S"""
    Parse the session's most recent agent message under the contract its
    last typed turn was sent with, and return the typed value.

    Separate from sending, rather than a reply to it, because a turn takes
    minutes: the client polls `Request::Updates` as it would for any
    session and asks for the answer once the turn completes. It reads the
    same journal the interface renders, so it works on a live interaction
    and a stored session alike — an answer can be re-parsed long after the
    interaction it came from has ended.

    Fields of `data`:

      * `id      `  string
      * `contract`  Contract|null  (optional)
    """
    def turn_answer(data), do: Styra.Protocol.build("turn_answer", data)

    @doc "`turn_answer/1`, raising on a request the server would refuse."
    def turn_answer!(data), do: Styra.Protocol.build!("turn_answer", data)

    @doc ~S"""
    Read the server's in-memory log of plan-quota readings seen on any
    interaction's wire, oldest first. Server-wide because quota belongs to
    the account rather than to one session, and in-memory because it is a
    live reading rather than a record: it starts empty with the daemon.
    """
    def quota_log, do: Styra.Protocol.build("quota_log")

    @doc "`quota_log/0`, raising on a request the server would refuse."
    def quota_log!, do: Styra.Protocol.build!("quota_log")

    @doc ~S"""
    Ask the server to remove its socket and exit. Any live interactions it owns die
    with it, so this is the deliberate counterpart to the daemon outliving
    its clients.
    """
    def shutdown, do: Styra.Protocol.build("shutdown")

    @doc "`shutdown/0`, raising on a request the server would refuse."
    def shutdown!, do: Styra.Protocol.build!("shutdown")
  end
end

defmodule Styra.Protocol.Response do
  @moduledoc ~S"""
  Wire spellings of `Response`.

  Versioned request envelope. Flattening keeps `operation` at the top level.
  Successful response payload. The variant must match the request operation.
  """

  @spellings [
    {:health, "health"},
    {:workspace_created, "workspace_created"},
    {:workspaces, "workspaces"},
    {:workspace, "workspace"},
    {:workspace_git_repository_updated, "workspace_git_repository_updated"},
    {:workspace_worktrees_updated, "workspace_worktrees_updated"},
    {:workspace_launch, "workspace_launch"},
    {:session_created, "session_created"},
    {:session_plan, "session_plan"},
    {:templates, "templates"},
    {:session_resumed, "session_resumed"},
    {:session_converted, "session_converted"},
    {:session_branched, "session_branched"},
    {:session_renamed, "session_renamed"},
    {:workspace_launch_updated, "workspace_launch_updated"},
    {:accepted, "accepted"},
    {:queued, "queued"},
    {:sent_queued_message, "sent_queued_message"},
    {:queued_messages, "queued_messages"},
    {:interaction_loaded, "interaction_loaded"},
    {:updates, "updates"},
    {:interactions, "interactions"},
    {:stored_sessions, "stored_sessions"},
    {:stored_session, "stored_session"},
    {:provider_raw, "provider_raw"},
    {:shell, "shell"},
    {:answer, "answer"},
    {:quota_log, "quota_log"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def health, do: "health"

  def workspace_created, do: "workspace_created"

  def workspaces, do: "workspaces"

  def workspace, do: "workspace"

  def workspace_git_repository_updated, do: "workspace_git_repository_updated"

  def workspace_worktrees_updated, do: "workspace_worktrees_updated"

  def workspace_launch, do: "workspace_launch"

  def session_created, do: "session_created"

  def session_plan, do: "session_plan"

  def templates, do: "templates"

  def session_resumed, do: "session_resumed"

  def session_converted, do: "session_converted"

  def session_branched, do: "session_branched"

  def session_renamed, do: "session_renamed"

  def workspace_launch_updated, do: "workspace_launch_updated"

  def accepted, do: "accepted"

  def queued, do: "queued"

  def sent_queued_message, do: "sent_queued_message"

  def queued_messages, do: "queued_messages"

  def interaction_loaded, do: "interaction_loaded"

  def updates, do: "updates"

  def interactions, do: "interactions"

  def stored_sessions, do: "stored_sessions"

  def stored_session, do: "stored_session"

  def provider_raw, do: "provider_raw"

  def shell, do: "shell"

  def answer, do: "answer"

  def quota_log, do: "quota_log"
end

defmodule Styra.Protocol.WireResponse do
  @moduledoc ~S"""
  Wire spellings of `WireResponse`.

  Response envelope returned for every syntactically valid connection.
  """

  @spellings [
    {:ok, "ok"},
    {:error, "error"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def ok, do: "ok"

  def error, do: "error"
end

defmodule Styra.Protocol.BranchHistory do
  @moduledoc ~S"""
  Wire spellings of `BranchHistory`.

  The source history used to seed a branch.
  """

  @spellings [
    {:through_selected, "through_selected"},
    {:selected_only, "selected_only"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  Copy the source from its beginning through the selected entry.
  """
  def through_selected, do: "through_selected"

  @doc ~S"""
  Copy only the selected entry.
  """
  def selected_only, do: "selected_only"
end

defmodule Styra.Protocol.Provider do
  @moduledoc ~S"""
  Wire spellings of `Provider`.

  Which coding agent a session launches, and thus which command line and wire
  protocol it gets. The model and reasoning effort are chosen separately (see
  `Selection`); a provider is only the agent itself.
  """

  @spellings [
    {:codex, "codex"},
    {:"codex-exec", "codex-exec"},
    {:claude, "claude"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  Multi-turn codex over the `app-server` JSON-RPC protocol.
  """
  def codex, do: "codex"

  @doc ~S"""
  Multi-turn Claude Code over bidirectional `stream-json`.
  """
  def claude, do: "claude"
end

defmodule Styra.Protocol.WorkspaceLaunchChange do
  @moduledoc ~S"""
  Wire spellings of `WorkspaceLaunchChange`.

  One server-owned edit to a Workspace's standing launch policy.

  Clients send intent instead of replacing a locally cached copy. This lets
  the server apply the edit to the latest stored policy and return the
  authoritative result, avoiding lost updates between multiple Styra UIs.
  """

  @spellings [
    {:set_network, "set_network"},
    {:set_writable_workspace, "set_writable_workspace"},
    {:set_templates, "set_templates"},
    {:add_mounts, "add_mounts"},
    {:remove_mount, "remove_mount"},
    {:replace, "replace"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def set_network, do: "set_network"

  def set_writable_workspace, do: "set_writable_workspace"

  def set_templates, do: "set_templates"

  def add_mounts, do: "add_mounts"

  def remove_mount, do: "remove_mount"

  def replace, do: "replace"
end

defmodule Styra.Protocol.Contract do
  @moduledoc ~S"""
  Wire spellings of `Contract`.

  The shape a client asks a turn's answer to come back in.

  A contract is applied at both ends of one turn: it frames the message sent
  to the agent with instructions describing the shape, and it parses the
  agent's reply back into `AnswerValue`. Framing server-side is what keeps
  clients honest — every caller asks for a shape the same way, so the parser
  only has to understand one phrasing. See `crate::contract`.
  """

  @spellings [
    {:text, "text"},
    {:lines, "lines"},
    {:files, "files"},
    {:json, "json"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  Prose. The weakest contract, and the one that cannot fail to parse.
  """
  def text, do: "text"

  @doc ~S"""
  One item per line.
  """
  def lines, do: "lines"

  @doc ~S"""
  One file location per line, `path[:line[:column]][: description]`.
  """
  def files, do: "files"

  @doc ~S"""
  A single JSON value.
  """
  def json, do: "json"
end

defmodule Styra.Protocol.Effort do
  @moduledoc ~S"""
  Wire spellings of `Effort`.

  How much reasoning the model is asked to spend per turn.

  One vocabulary across providers, since the ladders coincide in the middle;
  `Provider::efforts` narrows it to what a given agent accepts. Passed to
  codex as its `model_reasoning_effort` config override and to Claude Code as
  `--effort`.
  """

  @spellings [
    {:minimal, "minimal"},
    {:low, "low"},
    {:medium, "medium"},
    {:high, "high"},
    {:xhigh, "xhigh"},
    {:max, "max"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def minimal, do: "minimal"

  def low, do: "low"

  def medium, do: "medium"

  def high, do: "high"

  def xhigh, do: "xhigh"

  def max, do: "max"
end

defmodule Styra.Protocol.InteractionActivity do
  @moduledoc ~S"""
  Wire spellings of `InteractionActivity`.

  What a live interaction is currently waiting on.
  """

  @spellings [
    {:pending, "pending"},
    {:running, "running"},
    {:background, "background"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  The agent is idle and waiting for the operator's next message.
  """
  def pending, do: "pending"

  @doc ~S"""
  The agent is working on an operator message.
  """
  def running, do: "running"

  @doc ~S"""
  The agent is waiting for input while a background task is active.
  """
  def background, do: "background"
end

defmodule Styra.Protocol.AgentEvent do
  @moduledoc ~S"""
  Wire spellings of `AgentEvent`.

  The stable, provider-independent event vocabulary.
  """

  @spellings [
    {:user_message, "user_message"},
    {:thread_started, "thread_started"},
    {:turn_started, "turn_started"},
    {:turn_completed, "turn_completed"},
    {:usage_updated, "usage_updated"},
    {:command_started, "command_started"},
    {:command_completed, "command_completed"},
    {:file_changed, "file_changed"},
    {:diff_updated, "diff_updated"},
    {:tool_started, "tool_started"},
    {:tool_completed, "tool_completed"},
    {:plan_updated, "plan_updated"},
    {:agent_message, "agent_message"},
    {:thinking, "thinking"},
    {:error, "error"},
    {:model_changed, "model_changed"},
    {:branched, "branched"},
    {:task_started, "task_started"},
    {:task_progress, "task_progress"},
    {:task_completed, "task_completed"},
    {:background_tasks, "background_tasks"},
    {:unknown, "unknown"},
    {:malformed, "malformed"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  A message the operator sent to the agent, recorded so their own turns
  appear inline in the same list. Host-originated, never decoded.
  """
  def user_message, do: "user_message"

  @doc ~S"""
  A session began. Both agents report what they are actually running as
  they start one, so this is also where the effective model and reasoning
  effort come from — not from what the operator asked for, which may have
  named neither. `model` and `effort` are `None` when the agent's start
  line does not name them (Claude Code reports a model but no effort; a
  codex `thread/started` notification names neither, and its
  `thread/start` response names both).
  """
  def thread_started, do: "thread_started"

  def turn_started, do: "turn_started"

  def turn_completed, do: "turn_completed"

  @doc ~S"""
  A token-usage snapshot that arrives independently of a turn's end (the
  app-server protocol reports it after every step within a turn, not just
  the last). Updates the usage display without signalling that the agent
  has gone idle — see `TurnCompleted` for the actual end-of-turn signal.
  """
  def usage_updated, do: "usage_updated"

  def command_started, do: "command_started"

  def command_completed, do: "command_completed"

  def file_changed, do: "file_changed"

  @doc ~S"""
  An aggregated diff snapshot that is not itself a file-change item.
  Codex app-server emits this after each file-change item.
  """
  def diff_updated, do: "diff_updated"

  def tool_started, do: "tool_started"

  def tool_completed, do: "tool_completed"

  def plan_updated, do: "plan_updated"

  def agent_message, do: "agent_message"

  @doc ~S"""
  Claude's extended-thinking prose, surfaced only when a message carries
  no visible text alongside it — see `AgentEvent::is_minor`. Claude also
  reports its thinking-token spend on its own lines, with no prose; such
  an update carries only `tokens`, and clients fold consecutive thinking
  events into one line rather than showing every tick (see
  `AgentEvent::updates_thinking`).
  """
  def thinking, do: "thinking"

  def error, do: "error"

  @doc ~S"""
  The operator moved the session onto a different model or reasoning
  effort mid-conversation. No provider reports this on the wire — the
  host synthesizes it when it applies the change — but it belongs in the
  log beside the messages, because which model answered is part of
  reading the conversation back.

  Each field is `Some` only if *that* setting changed, so the line states
  the change rather than restating the whole selection: an operator who
  switched model alone should not have to remember what the effort was to
  see that it stayed put. At least one is always `Some` — a selection
  equal to the current one is not a change and produces no event.
  """
  def model_changed, do: "model_changed"

  @doc ~S"""
  A branch boundary between two sessions. No provider reports this — the
  host writes one marker into each side when it branches a session: the
  source records where its history was continued, the branch records
  where its history came from. It names the *other* session, so a client
  can follow the marker and open it.
  """
  def branched, do: "branched"

  @doc ~S"""
  A task the agent started alongside its own work: a backgrounded shell
  command or a subagent. Claude keys these by a task id that its later
  progress and completion lines repeat, so a client shows one row per
  task rather than one per report — see `AgentEvent::task_id`.
  """
  def task_started, do: "task_started"

  @doc ~S"""
  What a running task is doing now, reported repeatedly while it runs.
  `description` is the task's current activity, not the description it
  started with.
  """
  def task_progress, do: "task_progress"

  @doc ~S"""
  A task reached a terminal state. Claude reports this twice for the same
  task — once as a notification carrying a human summary, once as a status
  patch that may carry the error — so clients merge both into one row.
  """
  def task_completed, do: "task_completed"

  @doc ~S"""
  The authoritative count of background tasks the agent currently has
  running, as reported by the provider whenever that set changes.
  """
  def background_tasks, do: "background_tasks"

  @doc ~S"""
  A recognised envelope with no rendered view; carried, not shown as prose.
  """
  def unknown, do: "unknown"

  @doc ~S"""
  An undecodable line, kept visible as an error rather than dropped.
  """
  def malformed, do: "malformed"
end

defmodule Styra.Protocol.AnswerValue do
  @moduledoc ~S"""
  Wire spellings of `AnswerValue`.

  A parsed answer. The variant always matches the `Contract` that produced
  it, so a client can dispatch on the value alone.
  """

  @spellings [
    {:text, "text"},
    {:lines, "lines"},
    {:files, "files"},
    {:json, "json"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def text, do: "text"

  def lines, do: "lines"

  def files, do: "files"

  def json, do: "json"
end

defmodule Styra.Protocol.QuotaStatus do
  @moduledoc ~S"""
  Wire spellings of `QuotaStatus`.

  How much of a quota window is left, as the provider judges it.
  """

  @spellings [
    {:allowed, "allowed"},
    {:warning, "warning"},
    {:exhausted, "exhausted"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  Room remains, and the provider is not warning about the window.
  """
  def allowed, do: "allowed"

  @doc ~S"""
  The window is close enough to full that the provider says so, or its
  usage has passed `crate::quota::WARN_THRESHOLD`.
  """
  def warning, do: "warning"

  @doc ~S"""
  The window is full: turns are being refused until it resets.
  """
  def exhausted, do: "exhausted"
end

defmodule Styra.Protocol.MountOrigin do
  @moduledoc ~S"""
  Wire spellings of `MountOrigin`.

  Which layer of the launch policy put a mount in the sandbox.

  The effective policy is one flat list by the time Driva runs it, but the
  layers that produced it are not interchangeable to an operator: a grant from
  the agent profile is a property of the agent they picked, one from a
  template is a property of the template, and one they typed is theirs to take
  back. Recording the layer at the point the list is built is the only place
  the answer is known for certain — afterwards a mount is just a path pair.
  """

  @spellings [
    {:workspace, "workspace"},
    {:git_repository, "git_repository"},
    {:scratch, "scratch"},
    {:profile, "profile"},
    {:template, "template"},
    {:tooling, "tooling"},
    {:operator, "operator"},
    {:broker, "broker"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  The operator's project, bound writable as the agent's workspace.
  """
  def workspace, do: "workspace"

  @doc ~S"""
  The Git checkout durably associated with the Workspace.
  """
  def git_repository, do: "git_repository"

  @doc ~S"""
  An empty writable filesystem discarded when the run ends.
  """
  def scratch, do: "scratch"

  @doc ~S"""
  Granted by the agent profile — its credentials, tools, and caches.
  """
  def profile, do: "profile"

  @doc ~S"""
  Granted by one of the selected Driva templates.
  """
  def template, do: "template"

  @doc ~S"""
  A host executable the launch has to run — the agent itself, or the
  `tmux` the session shell needs — that the private root does not carry.
  """
  def tooling, do: "tooling"

  @doc ~S"""
  Asked for by hand, through the launch policy's mount key.
  """
  def operator, do: "operator"

  @doc ~S"""
  The hidden control mount the sandbox broker needs for its tmux shell.
  """
  def broker, do: "broker"
end

defmodule Styra.Protocol.Mount do
  @moduledoc ~S"""
  Wire spellings of `Mount`.

  A filesystem made available inside an isolated execution.
  """

  @spellings [
    {:bind, "bind"},
    {:temporary, "temporary"},
    {:overlay, "overlay"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  A host path exposed at an isolated destination.
  """
  def bind, do: "bind"

  @doc ~S"""
  An empty, writable filesystem discarded after execution.
  """
  def temporary, do: "temporary"

  @doc ~S"""
  Host content exposed read-only as the lower layer, writable through an
  invisible tmpfs upper layer. Writes are visible for the life of the
  process and discarded when it exits; the host source is never
  mutated.

  Overlayfs stacks only on directories, so a backend given a file source
  reproduces the same semantics another way — Bubblewrap binds a private
  copy of the file and removes it after execution.
  """
  def overlay, do: "overlay"
end

defmodule Styra.Protocol.InteractionUpdate do
  @moduledoc ~S"""
  Wire spellings of `InteractionUpdate`.

  An update delivered from the interaction's threads to the UI.
  """

  @spellings [
    {:event, "event"},
    {:raw, "raw"},
    {:log, "log"},
    {:quota, "quota"},
    {:working_directory_changed, "working_directory_changed"},
    {:ended, "ended"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  A decoded agent event or an operator message, in occurrence order.
  """
  def event, do: "event"

  @doc ~S"""
  One verbatim wire line, for the raw-interaction view.
  """
  def raw, do: "raw"

  @doc ~S"""
  A diagnostic message for the log view.
  """
  def log, do: "log"

  @doc ~S"""
  A plan-quota window worth the operator's attention — nearly full, or
  full. Only readings that say something new are sent; every reading,
  notable or not, is kept in the server's quota log for
  `crate::protocol::Request::QuotaLog` to return.
  """
  def quota, do: "quota"

  @doc ~S"""
  The host directory used for subsequent agent turns.
  """
  def working_directory_changed, do: "working_directory_changed"

  @doc ~S"""
  The agent process ended; no further events will arrive.
  """
  def ended, do: "ended"
end

defmodule Styra.Protocol.BranchDirection do
  @moduledoc ~S"""
  Wire spellings of `BranchDirection`.

  Which side of a branch an `AgentEvent::Branched` marker sits on. Each
  marker names the other session, so the direction says which way the link
  points rather than which session it belongs to.
  """

  @spellings [
    {:from, "from"},
    {:to, "to"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  This session was branched from the named one.
  """
  def from, do: "from"

  @doc ~S"""
  The named session was branched from this one.
  """
  def to, do: "to"
end

defmodule Styra.Protocol.Direction do
  @moduledoc ~S"""
  Wire spellings of `Direction`.

  Which way a wire line travelled.
  """

  @spellings [
    {:to_agent, "to_agent"},
    {:from_agent, "from_agent"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  @doc ~S"""
  A line Styra wrote to the agent's stdin.
  """
  def to_agent, do: "to_agent"

  @doc ~S"""
  A line received on the agent's stdout.
  """
  def from_agent, do: "from_agent"
end

defmodule Styra.Protocol.MountAccess do
  @moduledoc ~S"""
  Wire spellings of `MountAccess`.
  """

  @spellings [
    {:readonly, "readonly"},
    {:readwrite, "readwrite"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def readonly, do: "readonly"

  def readwrite, do: "readwrite"
end

defmodule Styra.Protocol.LogLevel do
  @moduledoc ~S"""
  Wire spellings of `LogLevel`.

  Severity of a `LogEntry`, used to colour the log view.
  """

  @spellings [
    {:info, "info"},
    {:warn, "warn"},
    {:error, "error"}
  ]

  @doc "Every spelling as `{atom, wire}`, in declaration order."
  def spellings, do: @spellings

  @doc "Every wire spelling, in declaration order."
  def values, do: Enum.map(@spellings, &elem(&1, 1))

  @doc "The wire spelling of an atom, or nil."
  def spelling(atom) do
    case List.keyfind(@spellings, atom, 0) do
      {_atom, wire} -> wire
      nil -> nil
    end
  end

  @doc "The atom for a wire spelling: `{:ok, atom}` or `:error`."
  def parse(wire) do
    case List.keyfind(@spellings, wire, 1) do
      {atom, _wire} -> {:ok, atom}
      nil -> :error
    end
  end

  def info, do: "info"

  def warn, do: "warn"

  def error, do: "error"
end
