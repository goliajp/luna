-- v3.1 string slice: string.format in 5.1 (C casts, strlen'd items, 5.1 %q).
-- Only in-range casts and flags every libc agrees on: the rest is
-- platform-defined in PUC.
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
P("d cast", function() return F("%d|%5.1d|%+i", 3.9, -2.5, 2^40) end)
P("x cast", function() return F("%x|%X|%o|%u", 0.5, 255.9, 8.5, 2^53) end)
P("c zero", function() return F("[%c]", 0), F("[%3c]", 0), F("[%-3c]", 0) end)
P("c cast", function() return F("%c%c", 65.9, 322) end)
P("s zero", function() return F("[%s]", "a\0b"), F("[%5.2s]", "a\0b") end)
P("s long", function() return #F("%5s", string.rep("y", 150)), #F("%.3s", string.rep("y", 150)) end)
P("s table", function() return F("%s", {}) end)
P("q", function() return F("%q", "a\0b\r\n\"\\\1\2552") end)
P("q number", function() return F("%q", 1/3) end)
P("a", function() return F("%a", 1) end)
P("flags", function() return F("%-05d|%#x|%#o|% d|%+u", 3, 255, 8, 7, 3) end)
P("repeated flags", function() return F("%-----5d", 1) end)
P("repeated flags 6", function() return F("%------5d", 1) end)
P("too long", function() return F("%123d", 1) end)
P("bad option", function() return F("%y", 1) end)
P("trailing", function() return F("abc%", 1) end)
P("no value", function() return F("%d %d", 1) end)
P("g", function() return F("%g|%G|%.3g|%#g|%10.4g", 1e-5, 1e20, 0.0001234, 3, 1234567) end)
P("e", function() return F("%e|%.0e|%#.0e|%E", 12345.678, 2.5, 3, 1e-300) end)
P("f", function() return F("%.0f|%.2f|%010.3f|%-8.1f|%+f", 2.5, 1.005, -3.14159, 2.25, 0) end)
P("inf", function() return F("%f|%5.1f|%-6e|%+g|%05f", 1/0, -1/0, 1/0, 1/0, 1/0) end)
