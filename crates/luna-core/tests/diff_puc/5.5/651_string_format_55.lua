-- v3.1 string slice: string.format in 5.5 (checkformat, quotefloat, %p).
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
P("d", function() return F("%d|%5.3d|%+i|%x|%#X|%-5u|", 3, -2, 7, 255, 255, 3) end)
P("d string", function() return F("%d|%x", "10", "0x10") end)
P("c", function() return F("[%c]", 0), F("%-5c|", 65) end)
P("q float", function() return F("%q|%q|%q|%q", 0.5, 3.0, -0.0, 2^63) end)
P("q special", function() return F("%q|%q|%q", 1/0, -1/0, 0/0) end)
P("q modifiers", function() return F("%5q", "x") end)
P("s", function() return F("%s|%.2s|%5s", 1.0, true, nil) end)
P("s zero width", function() return F("[%5s]", "a\0b") end)
P("s tostring number", function() return F("%s", setmetatable({}, {__tostring = function() return 42 end})) end)
P("p", function() return F("%p|%5p|%-7p|", 1, nil, true), F("%p", {}) ~= F("%p", {}) end)
P("spec flags", function() return F("%#d", 1) end)
P("spec flags 2", function() return F("%+x", 1) end)
P("spec s", function() return F("%05s", "a") end)
P("spec c", function() return F("%.3c", 65) end)
P("spec width", function() return F("%100d", 1) end)
P("spec zero width", function() return F("%-07.2d|%00005d", 1, 2) end)
P("too long", function() return F("%---------------------d", 1) end)
P("bad conversion", function() return F("%5.2y", 1) end)
P("trailing", function() return F("abc%", 1) end)
P("a", function() return F("%a|%A|%.3a|%+a", 1, 0.5, 1, 2) end)
