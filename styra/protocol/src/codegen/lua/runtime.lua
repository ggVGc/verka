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
