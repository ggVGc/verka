-- Svara: Styra from inside Neovim.
--
--   local svara = require("svara")
--   local styra = assert(svara.open())
--   local interactions = assert(styra:interactions())
--   assert(svara.send_message("styra-7", "review this buffer"))
--
-- `svara.api` is the whole of it — one function per interaction with the
-- server, over the generated protocol library. What is here is the front door:
-- `open`, the value helpers worth having at hand, and the one-shot
-- `send_message` this plugin began as.

local api = require("svara.api")

local M = {}

--- The API module itself, and the vocabulary underneath it.
M.api = api
M.protocol = api.protocol

--- Open a client; see `svara.api.open`.
M.open = api.open

--- A nullable field read as an absence; see `svara.api.given`.
M.given = api.given

--- A selection from a profile name or a table; see `svara.api.selection`.
M.selection = api.selection
M.selection_name = api.selection_name

--- The value an `Answer` carries; see `svara.api.answered`.
M.answered = api.answered

--- Send one message to a live session; see `svara.core`.
M.send_message = require("svara.core").send_message

--- Start an interaction in the Workspace over a directory; see `svara.core`.
M.start = require("svara.core").start

--- List live interactions in the Workspace over a directory; see `svara.core`.
M.interactions_for_directory = require("svara.core").interactions_for_directory

--- Remember or read this Neovim session's interaction choice for a Workspace.
M.select_interaction = require("svara.core").select_interaction
M.selected_interaction = require("svara.core").selected_interaction

--- Send a message to the selected interaction in the Workspace over a directory.
M.send_to_selected = require("svara.core").send_to_selected

--- Ask the selected interaction for file locations, as quickfix items; see
--- `svara.core`.
M.find = require("svara.core").find

--- What Svara would do in a directory — Workspace, model, selected
--- interaction — as a table, and as lines to show; see `svara.core`.
M.info = require("svara.core").info
M.info_lines = require("svara.core").info_lines

return M
