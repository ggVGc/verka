-- A small JSON codec, so the example runs on a machine with no Lua JSON
-- library installed.
--
-- Not part of the protocol and not what a real client should use: prefer
-- lua-cjson or dkjson, which `styra.client` picks up when either is present.
-- This is here so the example is runnable as it stands rather than after a
-- detour through a package manager.
--
-- One ambiguity is worth knowing about, because Lua cannot express it: an empty
-- table is written as `{}`, an empty object. The protocol's empty lists are all
-- optional fields, so leave them out rather than passing an empty table.

local M = {}

--- The value meaning JSON null, since Lua's nil cannot survive in a table.
M.null = setmetatable({}, {
  __tostring = function()
    return "null"
  end,
})

-- Encoding ---------------------------------------------------------------

local escapes = {
  ['"'] = '\\"',
  ["\\"] = "\\\\",
  ["\b"] = "\\b",
  ["\f"] = "\\f",
  ["\n"] = "\\n",
  ["\r"] = "\\r",
  ["\t"] = "\\t",
}

local function encode_string(value)
  return '"'
    .. value:gsub('[%z\1-\31\\"]', function(character)
      return escapes[character] or string.format("\\u%04x", character:byte())
    end)
    .. '"'
end

local function is_array(value)
  local count = 0
  for key in pairs(value) do
    if type(key) ~= "number" then
      return false
    end
    count = count + 1
  end
  return count == #value and count > 0
end

local function encode(value)
  if value == M.null or value == nil then
    return "null"
  end
  local kind = type(value)
  if kind == "string" then
    return encode_string(value)
  end
  if kind == "number" then
    if value % 1 == 0 then
      return string.format("%d", value)
    end
    return string.format("%.14g", value)
  end
  if kind == "boolean" then
    return tostring(value)
  end
  if kind ~= "table" then
    error("cannot encode a " .. kind)
  end
  local parts = {}
  if is_array(value) then
    for _, item in ipairs(value) do
      parts[#parts + 1] = encode(item)
    end
    return "[" .. table.concat(parts, ",") .. "]"
  end
  for key, item in pairs(value) do
    parts[#parts + 1] = encode_string(tostring(key)) .. ":" .. encode(item)
  end
  return "{" .. table.concat(parts, ",") .. "}"
end

M.encode = encode

-- Decoding ---------------------------------------------------------------

local decode_value

local function skip_space(text, position)
  local _, stop = text:find("^[ \t\r\n]*", position)
  return stop + 1
end

local function expect(text, position, character)
  if text:sub(position, position) ~= character then
    error(string.format("expected %q at %d, got %q", character, position, text:sub(position, position)))
  end
  return position + 1
end

local unescapes = {
  ['"'] = '"',
  ["\\"] = "\\",
  ["/"] = "/",
  b = "\b",
  f = "\f",
  n = "\n",
  r = "\r",
  t = "\t",
}

local function decode_string(text, position)
  position = expect(text, position, '"')
  local parts = {}
  while true do
    local character = text:sub(position, position)
    if character == "" then
      error("unterminated string")
    end
    if character == '"' then
      return table.concat(parts), position + 1
    end
    if character == "\\" then
      local escaped = text:sub(position + 1, position + 1)
      if escaped == "u" then
        local code = tonumber(text:sub(position + 2, position + 5), 16)
        -- Enough for the text these payloads carry; a real codec would also
        -- rejoin surrogate pairs.
        parts[#parts + 1] = utf8 and utf8.char(code) or string.char(code % 256)
        position = position + 6
      else
        parts[#parts + 1] = unescapes[escaped] or escaped
        position = position + 2
      end
    else
      parts[#parts + 1] = character
      position = position + 1
    end
  end
end

local function decode_array(text, position)
  position = skip_space(text, expect(text, position, "["))
  local items = {}
  if text:sub(position, position) == "]" then
    return items, position + 1
  end
  while true do
    local item
    item, position = decode_value(text, position)
    items[#items + 1] = item
    position = skip_space(text, position)
    local character = text:sub(position, position)
    if character == "]" then
      return items, position + 1
    end
    position = skip_space(text, expect(text, position, ","))
  end
end

local function decode_object(text, position)
  position = skip_space(text, expect(text, position, "{"))
  local fields = {}
  if text:sub(position, position) == "}" then
    return fields, position + 1
  end
  while true do
    local key, value
    key, position = decode_string(text, skip_space(text, position))
    position = skip_space(text, expect(text, skip_space(text, position), ":"))
    value, position = decode_value(text, position)
    fields[key] = value
    position = skip_space(text, position)
    local character = text:sub(position, position)
    if character == "}" then
      return fields, position + 1
    end
    position = skip_space(text, expect(text, position, ","))
  end
end

function decode_value(text, position)
  position = skip_space(text, position)
  local character = text:sub(position, position)
  if character == "{" then
    return decode_object(text, position)
  end
  if character == "[" then
    return decode_array(text, position)
  end
  if character == '"' then
    return decode_string(text, position)
  end
  if text:sub(position, position + 3) == "true" then
    return true, position + 4
  end
  if text:sub(position, position + 4) == "false" then
    return false, position + 5
  end
  if text:sub(position, position + 3) == "null" then
    return M.null, position + 4
  end
  local literal = text:match("^-?%d+%.?%d*[eE]?[-+]?%d*", position)
  local number = literal and tonumber(literal)
  if not number then
    error(string.format("unreadable JSON at %d: %q", position, text:sub(position, position + 20)))
  end
  return number, position + #literal
end

--- Decode one JSON document, or nil and a message.
function M.decode(text)
  local ok, value = pcall(function()
    local decoded, position = decode_value(text, 1)
    position = skip_space(text, position)
    if position <= #text then
      error(string.format("trailing text at %d", position))
    end
    return decoded
  end)
  if not ok then
    return nil, tostring(value)
  end
  return value
end

return M
