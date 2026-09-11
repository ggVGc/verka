-- Exercises the generated library from the language it is generated for.
-- Run by `the_generated_library_runs_against_a_real_interpreter`, against a
-- freshly generated copy, so a generator that emits plausible-looking Lua that
-- does not actually work is caught here rather than by a client.

local protocol = require("styra.protocol")

local function ok(condition, message)
  if not condition then
    error(message, 2)
  end
end

local function refuses(build, expected)
  local fine, err = pcall(build)
  ok(not fine, "expected a refusal: " .. tostring(expected))
  ok(
    tostring(err):find(expected, 1, true) ~= nil,
    string.format("expected %q in %q", expected, tostring(err))
  )
end

-- A request with no data is the operation alone.
local health = protocol.request.health()
ok(health.operation == "health", "health names its operation")
ok(health.data == nil, "health carries no data")

-- A request with data checks it on the way out.
local created = protocol.request.create_session({
  workspace_id = "w-1",
  selection = { provider = protocol.Provider.CLAUDE, model = "claude-opus-5", effort = "xhigh" },
  message = "hello",
  contract = protocol.Contract.LINES,
})
ok(created.operation == "create_session", "the operation is named")
ok(created.data.workspace_id == "w-1", "the data is carried through untouched")

refuses(function()
  protocol.request.create_session({ selection = { provider = "claude", model = "m", effort = "high" } })
end, 'missing required field "workspace_id"')

refuses(function()
  protocol.request.create_session({
    workspace_id = "w-1",
    selection = { provider = "claude", model = "m", effort = "high" },
    workspce = "typo",
  })
end, 'unknown field "workspce"')

refuses(function()
  protocol.request.create_session({
    workspace_id = "w-1",
    selection = { provider = "gemini", model = "m", effort = "high" },
  })
end, '"gemini" is not one of')

refuses(function()
  protocol.request.updates({ id = "styra-1", after = "soon" })
end, "expected a number")

refuses(function()
  protocol.request.send_message({ id = "styra-1", message = { text = "hi" }, extra = true })
end, "unknown field")

refuses(function()
  protocol.build("no_such_operation", {})
end, "is not a Styra operation")

-- Optional fields may be left out, and nullable ones may be said explicitly.
local renamed = protocol.request.rename_session({ id = "styra-1", name = protocol.null })
ok(renamed.data.name == protocol.null, "the null survives to the encoder")
refuses(function()
  protocol.request.rename_session({ id = "styra-1" })
end, 'missing required field "name"')

-- Validation without the raise, for a client that would rather report it.
local fine, err = protocol.validate("LaunchPolicy", { network = true, templates = { "rust" } })
ok(fine, tostring(err))
fine, err = protocol.validate("LaunchPolicy", { network = "yes" })
ok(not fine and err:find("network") ~= nil, "the message names the field")

-- Internally tagged payloads: an agent event as it arrives in an update.
ok(protocol.validate("AgentEvent", { type = "agent_message", text = "done" }), "a tagged event")
fine, err = protocol.validate("AgentEvent", { type = "agent_message" })
ok(not fine, "a tagged event still needs its fields")
fine, err = protocol.validate("AgentEvent", { type = "invented" })
ok(not fine and err:find("invented") ~= nil, "an unknown event type is named")

-- Adjacently tagged payloads: one update off the stream.
ok(
  protocol.validate("InteractionUpdate", {
    type = protocol.InteractionUpdate.LOG,
    data = { level = protocol.LogLevel.INFO, message = "ready" },
  }),
  "a log update"
)

-- Responses, both ways round.
local response, error_message = protocol.unwrap({
  status = "ok",
  response = { type = "health", data = { service = "styra" } },
})
ok(response and response.data.service == "styra", "an ok response unwraps")
response, error_message = protocol.unwrap({ status = "error", error = "no such session" })
ok(response == nil and error_message == "no such session", "an error response carries its message")

local health_data = protocol.expect(
  { status = "ok", response = { type = "health", data = { service = "styra" } } },
  protocol.Response.HEALTH
)
ok(health_data.service == "styra", "expect returns the data")
local mismatch, mismatch_error = protocol.expect(
  { status = "ok", response = { type = "accepted" } },
  protocol.Response.HEALTH
)
ok(mismatch == nil and mismatch_error:find("accepted") ~= nil, "expect names the reply it got")

-- The vocabulary itself.
ok(#protocol.OPERATIONS > 30, "every operation is listed")
ok(protocol.Contract.FILES == "files", "contract spellings")
ok(protocol.ANSWER_OPEN == "<styra:answer>", "the answer delimiters travel with the protocol")
ok(protocol.types.SessionSummary.kind == "struct", "types are introspectable")

print("ok")
