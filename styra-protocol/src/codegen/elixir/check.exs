# Exercises the generated library from the language it is generated for. Run
# by `the_generated_library_runs_against_a_real_interpreter`, against a freshly
# generated copy, so a generator that emits plausible-looking Elixir that does
# not actually work is caught here rather than by a client.

alias Styra.Protocol
alias Styra.Protocol.Request

defmodule Check do
  def ok(true, _message), do: :ok
  def ok(:ok, _message), do: :ok
  def ok(condition, message), do: raise("#{message} (got #{inspect(condition)})")

  # A refusal has to say which field was wrong, not merely that something was.
  def refuses({:error, message}, expected) do
    if String.contains?(message, expected) do
      :ok
    else
      raise ~s(expected "#{expected}" in "#{message}")
    end
  end

  def refuses(other, expected), do: raise("expected a refusal (#{expected}), got #{inspect(other)}")
end

# A request with no data is the operation alone.
{:ok, health} = Request.health()
Check.ok(health == %{"operation" => "health"}, "health names its operation and carries no data")

# A request with data checks it on the way out. Atom keys are what writing
# Elixir by hand gives you, and they are taken as they are.
{:ok, created} =
  Request.create_session(%{
    workspace_id: "w-1",
    selection: %{provider: Protocol.Provider.claude(), model: "claude-opus-5", effort: "xhigh"},
    message: "hello",
    contract: Protocol.Contract.lines()
  })

Check.ok(created["operation"] == "create_session", "the operation is named")
Check.ok(created["data"].workspace_id == "w-1", "the data is carried through untouched")

# String keys are what a map read back off the wire has, and work the same.
{:ok, _} =
  Request.create_session(%{
    "workspace_id" => "w-1",
    "selection" => %{"provider" => "claude", "model" => "m", "effort" => "high"}
  })

Check.refuses(
  Request.create_session(%{selection: %{provider: "claude", model: "m", effort: "high"}}),
  ~s(missing required field "workspace_id")
)

Check.refuses(
  Request.create_session(%{
    workspace_id: "w-1",
    selection: %{provider: "claude", model: "m", effort: "high"},
    workspce: "typo"
  }),
  ~s(unknown field "workspce")
)

Check.refuses(
  Request.create_session(%{
    workspace_id: "w-1",
    selection: %{provider: "gemini", model: "m", effort: "high"}
  }),
  ~s("gemini" is not one of)
)

Check.refuses(Request.updates(%{id: "styra-1", after: "soon"}), "expected a number")
Check.refuses(Request.send_message(%{id: "s", message: %{text: "hi"}, extra: true}), "unknown field")
Check.refuses(Protocol.build("no_such_operation", %{}), "is not a Styra operation")

# The raising form is the same check with the other convention.
raised =
  try do
    Request.create_session!(%{})
    nil
  rescue
    error in ArgumentError -> error.message
  end

Check.ok(is_binary(raised) and String.contains?(raised, "workspace_id"), "the ! form raises the message")

# Nullable-but-required: Elixir needs no sentinel for this, unlike Lua. A key
# set to nil is present; a key left out is not.
{:ok, renamed} = Request.rename_session(%{id: "styra-1", name: nil})
Check.ok(Map.fetch(renamed["data"], :name) == {:ok, nil}, "the nil survives to the encoder")
Check.refuses(Request.rename_session(%{id: "styra-1"}), ~s(missing required field "name"))

# Validation without a request around it.
Check.ok(Protocol.validate("LaunchPolicy", %{network: true, templates: ["rust"]}), "a launch policy")
Check.refuses(Protocol.validate("LaunchPolicy", %{network: "yes"}), "network")

# Internally tagged payloads: an agent event as it arrives in an update.
Check.ok(Protocol.validate("AgentEvent", %{type: "agent_message", text: "done"}), "a tagged event")
Check.refuses(Protocol.validate("AgentEvent", %{type: "agent_message"}), "missing required field")
Check.refuses(Protocol.validate("AgentEvent", %{type: "invented"}), "invented")

# Adjacently tagged payloads: one update off the stream.
Check.ok(
  Protocol.validate("InteractionUpdate", %{
    type: Protocol.InteractionUpdate.log(),
    data: %{level: Protocol.LogLevel.info(), message: "ready"}
  }),
  "a log update"
)

# Responses, both ways round.
{:ok, response} =
  Protocol.unwrap(%{
    "status" => "ok",
    "response" => %{"type" => "health", "data" => %{"service" => "styra"}}
  })

Check.ok(response["data"]["service"] == "styra", "an ok response unwraps")

Check.refuses(Protocol.unwrap(%{"status" => "error", "error" => "no such session"}), "no such session")
Check.refuses(Protocol.unwrap(%{"status" => "ok"}), "carries no response")
Check.refuses(Protocol.unwrap("nonsense"), "not an object")

{:ok, health_data} =
  Protocol.expect(
    %{"status" => "ok", "response" => %{"type" => "health", "data" => %{"service" => "styra"}}},
    Protocol.Response.health()
  )

Check.ok(health_data["service"] == "styra", "expect returns the data")

Check.refuses(
  Protocol.expect(
    %{"status" => "ok", "response" => %{"type" => "accepted"}},
    Protocol.Response.health()
  ),
  "accepted"
)

# The vocabulary itself.
Check.ok(length(Protocol.operations()) > 30, "every operation is listed")
Check.ok(Protocol.Contract.files() == "files", "contract spellings")
Check.ok(Protocol.Contract.parse("files") == {:ok, :files}, "a spelling reads back as an atom")
Check.ok(Protocol.Contract.parse("nope") == :error, "an unknown spelling does not")
Check.ok("files" in Protocol.Contract.values(), "values lists them")
Check.ok(Protocol.answer_open() == "<styra:answer>", "the answer delimiters travel with the protocol")
Check.ok(Protocol.type("SessionSummary").kind == :struct, "types are introspectable")

IO.puts("ok")
