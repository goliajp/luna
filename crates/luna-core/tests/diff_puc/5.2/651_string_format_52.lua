-- v3.1 string slice: string.format in 5.2 (range-checked casts, luaL_tolstring, 5.2 %q).
-- Only in-range casts and flags every libc agrees on.
local function show(...)
  local t = {}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    if type(v) == "string" then
      v = string.format("%q", (v:gsub("^[^\n]-:%d+: ", ""))):gsub("\n", "n")
    end
    t[#t + 1] = tostring(v)
  end
  return table.concat(t, " ")
end
local function P(label, f, ...)
  print(label, show(pcall(f, ...)))
end
local F = string.format
P("d", function() return F("%d|%5.1d|%+i", 3.9, -2.5, 2^40) end)
P("d range", function() return F("%d", 2^80) end)
P("x", function() return F("%x|%X|%o|%u", 0.5, 255.9, 8.5, 2^53) end)
P("x negative", function() return F("%x", -1) end)
P("c", function() return F("[%c]", 0), F("%c", 65.9) end)
P("s tostring", function() return F("%s|%.2s", setmetatable({}, {__tostring = function() return "obj" end}), true) end)
P("s zero", function() return F("[%s]", "a\0b"), F("[%5.2s]", "a\0b") end)
P("s long", function() return #F("%5s", string.rep("y", 150)) end)
P("q", function() return F("%q", "a\0b\r\n\"\\\1\0012") end)
P("q number", function() return F("%q", 7) end)
P("q table", function() return F("%q", {}) end)
-- %a is left out: 5.2 accepts it only when built with LUA_USE_AFORMAT
-- (make linux / macosx define it, make posix does not)
P("repeated flags", function() return F("%------5d", 1) end)
P("too long", function() return F("%1.123f", 1) end)
P("bad option", function() return F("%y", 1) end)
P("no value", function() return F("%d %d", 1) end)
P("g", function() return F("%g|%.3g|%#g", 1e-5, 0.0001234, 3) end)
