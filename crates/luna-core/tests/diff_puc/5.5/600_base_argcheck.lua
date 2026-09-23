-- Base-library argument checks follow lbaselib: a missing argument is
-- "no value", checkany-style arguments must be present, and each dialect
-- words its type errors its own way.
-- Error messages are compared without their position ("@" marks one)
-- and without 5.2's hash-order-dependent "_G." qualifier.
local function clean(s)
  s = tostring(s):gsub("'_G%.", "'")
  local pos = s:match("^[^:\n]+:%d+: ") and "@" or ""
  return pos .. s:gsub("^[^:\n]+:%d+: ", "")
end
local function render(...)
  local out = {}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    local t = type(v)
    if t == "string" then out[#out + 1] = clean(v)
    elseif t == "number" or t == "boolean" or t == "nil" then out[#out + 1] = tostring(v)
    else out[#out + 1] = "<" .. t .. ">" end
  end
  return table.concat(out, " | ")
end
local function show(label, ...) print(label, render(pcall(...))) end
show("type()", type)
show("getmetatable()", getmetatable)
show("rawequal(1)", rawequal, 1)
show("rawequal()", rawequal)
show("rawget({})", rawget, {})
show("rawget()", rawget)
show("rawset({}, 1)", rawset, {}, 1)
show("rawset({})", rawset, {})
show("rawset nil key", rawset, {}, nil, 1)
show("rawset nan key", rawset, {}, 0/0, 1)
show("rawset from lua", function() rawset({}, nil, 1) end)
show("setmetatable({})", setmetatable, {})
show("setmetatable({}, 1)", setmetatable, {}, 1)
show("setmetatable io", setmetatable, io.stdout, {})
show("setmetatable locked bad mt", setmetatable, setmetatable({}, {__metatable = 1}), 1)
show("setmetatable locked", setmetatable, setmetatable({}, {__metatable = 1}), {})
show("next()", next)
show("pcall()", pcall)
show("xpcall(print)", xpcall, print)
show("xpcall(print, nil)", xpcall, print, nil)
show("xpcall(print, 1)", xpcall, print, 1)
show("xpcall callable handler", xpcall, error, setmetatable({}, {__call = function() return "h" end}))
show("tostring()", tostring)
show("tonumber()", tonumber)
show("select()", select)
show("select('x')", select, "x")
show("select(1.5)", select, 1.5, "a", "b")
show("select('2')", select, "2", "a", "b")
show("select('#x')", select, "#x", "a", "b")
show("select(0)", select, 0)
show("select(-3)", select, -3, "a", "b")
show("error level 'x'", error, "m", "x")
show("error level '0'", error, "m", "0")
show("error level 2.5", error, "m", 2.5)
show("error nil level {}", error, nil, {})
local rawlen = rawget(_G, "rawlen")
if rawlen then
  show("rawlen()", rawlen)
  show("rawlen(1)", rawlen, 1)
  show("rawlen io", rawlen, io.stdout)
else
  print("rawlen", "absent")
end
local warn = rawget(_G, "warn")
if warn then
  show("warn()", warn)
  show("warn({})", warn, {})
  show("warn('a', {})", warn, "a", {})
  show("warn('@x', 1)", warn, "@x", 1)
end
