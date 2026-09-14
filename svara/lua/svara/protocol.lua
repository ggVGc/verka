-- Where Svara finds the generated wire vocabulary.
--
-- `styra.protocol` is generated from the Serde definitions in `styra-protocol`
-- and lives beside them, which is the whole point of generating it: a copy
-- vendored here would be a second home for the protocol, and a second home is
-- where drift starts. Lua has no manifest to declare that dependency in, so
-- the dependency is a search path, and this module is where Svara sets it:
--
--   local protocol = require("svara.protocol")
--
-- It looks, in order, at what is already on the search path (a plugin manager
-- that put `lua/styra/protocol.lua` on Neovim's runtimepath needs nothing
-- else), at `$STYRA_PROTOCOL_LUA`, and at the sibling checkout beside this
-- repository, so a clone runs without being installed.

local here = debug.getinfo(1, "S").source:match("^@(.*)/[^/]+$") or "."
-- .../svara/lua/svara/protocol.lua -> .../svara
local root = here .. "/../.."

local function loaded()
  local found, module = pcall(require, "styra.protocol")
  if found then
    return module
  end
  return nil
end

local function search(directory)
  package.path = directory .. "/?.lua;" .. directory .. "/?/init.lua;" .. package.path
end

local protocol = loaded()

if not protocol then
  local configured = os.getenv("STYRA_PROTOCOL_LUA")
  if configured and configured ~= "" then
    search(configured)
    protocol = loaded()
  end
end

if not protocol then
  search(root .. "/../styra-protocol/lua")
  protocol = loaded()
end

if not protocol then
  error(
    "svara: cannot find the generated styra.protocol module. Put its directory "
      .. "on package.path or Neovim's runtimepath, or set STYRA_PROTOCOL_LUA to "
      .. "the `lua` directory of the styra-protocol crate (it holds "
      .. "styra/protocol.lua, and `cargo run -p styra-protocol --bin "
      .. "styra-codegen -- lua` regenerates it)."
  )
end

return protocol
