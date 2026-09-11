#!/usr/bin/env lua
--
-- A small Styra client, written against the generated protocol library.
--
--   lua examples/styra-ask.lua health
--   lua examples/styra-ask.lua workspaces
--   lua examples/styra-ask.lua interactions
--   lua examples/styra-ask.lua ask styra-1 which files handle auth?
--
-- It exists to show what the library does and does not do. Every operation
-- name, field name, and enum spelling below comes from `styra.protocol`, which
-- is generated from the Rust that defines the protocol — nothing here restates
-- the wire format, and a request this file gets wrong is refused on the line
-- that built it rather than by the server a round trip later.
--
-- The JSON codec and the socket are the two things the protocol leaves to its
-- callers, and `styra.client` is where this repository keeps them; see its
-- header for what it needs installed. Point this at a server with --socket
-- PATH; the default is $XDG_RUNTIME_DIR/styra/styra.sock, the same one
-- `styractl` uses.

-- Lua has no dependency to declare, so the dependency is a search path: the
-- client and its neighbours here, and the generated protocol where it is
-- generated, in `styra-protocol/lua`. Both are found relative to this script
-- so it runs from anywhere without being installed; a real project would set
-- LUA_PATH once instead, and the README says how.
local here = arg[0]:match("^(.*)/[^/]+$") or "."
package.path = table.concat({
  here .. "/../?.lua",
  here .. "/../../styra-protocol/lua/?.lua",
  package.path,
}, ";")

local client = require("styra.client")
local protocol = require("styra.protocol")
local given = client.given

-- Commands --------------------------------------------------------------

local commands = {}

function commands.health(styra)
  local health, err = styra:call(protocol.request.health(), protocol.Response.HEALTH)
  if not health then
    return nil, err
  end
  print(string.format("%s is up", health.service))
  return true
end

function commands.workspaces(styra)
  local workspaces, err =
    styra:call(protocol.request.list_workspaces(), protocol.Response.WORKSPACES)
  if not workspaces then
    return nil, err
  end
  for _, workspace in ipairs(workspaces) do
    print(string.format(
      "%s  %-30s %s  (%d sessions)",
      workspace.id,
      given(workspace.name) or "-",
      workspace.host_path,
      workspace.session_count
    ))
  end
  return true
end

function commands.interactions(styra)
  local interactions, err =
    styra:call(protocol.request.list_interactions(), protocol.Response.INTERACTIONS)
  if not interactions then
    return nil, err
  end
  for _, interaction in ipairs(interactions) do
    print(string.format(
      "%s  %-10s %s:%s/%s  %s",
      interaction.id,
      given(interaction.activity) or protocol.InteractionActivity.PENDING,
      interaction.selection.provider,
      interaction.selection.model,
      interaction.selection.effort,
      given(interaction.last_message) or ""
    ))
  end
  return true
end

--- Ask one question under a contract, wait for the turn, print the answer.
---
--- The three requests this takes are the protocol's own shape, not a quirk of
--- Lua: a connection carries one request, and a turn takes minutes, so the
--- answer is fetched rather than returned. `send_message` names the contract,
--- `updates` is polled until the turn ends, and `turn_answer` parses the
--- reply the server already has.
function commands.ask(styra, session, ...)
  local question = table.concat({ ... }, " ")
  if not session or question == "" then
    return nil, "usage: ask <session> <question...>"
  end

  local sent, err = styra:call(
    protocol.request.send_message({
      id = session,
      message = { text = question, contract = protocol.Contract.LINES },
    }),
    protocol.Response.ACCEPTED
  )
  if not sent then
    return nil, err
  end

  local after, ended = 0, false
  while not ended do
    local updates, updates_error = styra:call(
      -- The raw wire lines dominate a long interaction's volume and this
      -- client renders none of them.
      protocol.request.updates({ id = session, after = after, raw = false }),
      protocol.Response.UPDATES
    )
    if not updates then
      return nil, updates_error
    end
    after = updates.next
    for _, sequenced in ipairs(updates.updates) do
      local update = sequenced.update
      if update.type == protocol.InteractionUpdate.EVENT then
        local event = update.data
        if event.type == protocol.AgentEvent.TURN_COMPLETED then
          ended = true
        elseif event.type == protocol.AgentEvent.ERROR then
          return nil, event.message
        end
      elseif update.type == protocol.InteractionUpdate.ENDED then
        return nil, "the interaction ended before it answered"
      end
    end
    if not ended then
      os.execute("sleep 0.25")
    end
  end

  local answer, answer_error =
    styra:call(protocol.request.turn_answer({ id = session }), protocol.Response.ANSWER)
  if not answer then
    return nil, answer_error
  end
  -- A reply that missed its contract is still an answer: show what was said
  -- and why it could not be read, rather than nothing at all.
  if not given(answer.value) then
    io.stderr:write(string.format("the reply did not satisfy its contract: %s\n", answer.error))
    print(answer.source)
    return true
  end
  for _, item in ipairs(answer.value.value) do
    print(item)
  end
  return true
end

-- Entry point -----------------------------------------------------------

local function usage()
  io.stderr:write([[
usage: styra-ask.lua [--socket PATH] <command> [arguments]

  health                        check that the server is reachable
  workspaces                    list Workspaces
  interactions                  list live interactions
  ask <session> <question...>   ask one question and print the typed answer
]])
  os.exit(2)
end

local arguments, path = {}, nil
local index = 1
while arg[index] do
  if arg[index] == "--socket" then
    index = index + 1
    path = arg[index] or usage()
  else
    arguments[#arguments + 1] = arg[index]
  end
  index = index + 1
end

local command = commands[arguments[1] or ""]
if not command then
  usage()
end

local styra, open_error = client.open({ socket = path })
if not styra then
  io.stderr:write(tostring(open_error), "\n")
  os.exit(1)
end

local unpack_arguments = table.unpack or unpack
local ok, message = command(styra, unpack_arguments(arguments, 2))
if not ok then
  io.stderr:write(tostring(message), "\n")
  os.exit(1)
end
