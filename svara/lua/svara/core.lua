-- The one function Svara started as, kept as it was.
--
-- Sending a message to a live session is what the `:SvaraSend` command and the
-- `bin/svara` script do, and they did it here before there was an API. They
-- still call this; it is now one line of `svara.api`, so there is one socket
-- and one copy of the wire format rather than two.

local M = {}

---Send a message to an existing, live Styra session.
---@param session_id string
---@param message string
---@param options? { socket?: string, timeout?: integer }
---@return boolean? sent
---@return string? error
function M.send_message(session_id, message, options)
  options = options or {}
  local styra, err = require("svara.api").open(options)
  if not styra then
    return nil, err
  end
  return styra:send_message(session_id, message)
end

return M
