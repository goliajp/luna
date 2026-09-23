-- v3.1 string slice: string.format in 5.3 (exact integers, hex %q, 5.3 messages).
-- Only flags every libc agrees on.
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
P("d", function() return F("%d|%5.3d|%+i|%x|%#X", 3, -2, 7, 255, 255) end)
P("d float", function() return F("%d", 3.5) end)
P("d string", function() return F("%d|%x", "10", "0x10") end)
P("c", function() return F("[%c]", 0), F("%5c", 65) end)
P("s", function() return F("%s|%.2s|%5s", 1.0, true, nil) end)
P("s zero", function() return F("[%s]", "a\0b") end)
P("s zero width", function() return F("[%5s]", "a\0b") end)
P("s name", function() return (F("%s", setmetatable({}, {__name = "MyType"})):gsub("0x%x+", "ADDR")) end)
P("s tostring number", function() return F("%s", setmetatable({}, {__tostring = function() return 42 end})) end)
P("q float", function() return F("%q|%q|%q|%q", 0.5, 1.0, -0.0, 2^63) end)
P("q int", function() return F("%q|%q", 7, math.mininteger) end)
P("q misc", function() return F("%q|%q", true, nil) end)
P("q modifiers", function() return F("%5q", "x") end)
P("q table", function() return F("%q", {}) end)
P("a", function() return F("%a|%A|%.3a|%+a", 1, 0.5, 1, 2) end)
P("bad option", function() return F("%y|%", 1) end)
P("bad option 2", function() return F("%5.2y", 1) end)
P("trailing", function() return F("abc%", 1) end)
P("repeated flags", function() return F("%------5d", 1) end)
P("too long", function() return F("%1.123f", 1) end)
