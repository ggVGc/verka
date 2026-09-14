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

return M
