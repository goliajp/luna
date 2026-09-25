-- v3.1 string slice: utf8 in 5.5 (offset returns both ends of the character)
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
local U = utf8
P("charpattern", function() return U.charpattern == "[\0-\127\194-\253][\128-\191]*" end)
P("char ext", function() return #U.char(0x7FFFFFFF) end)
P("char over", function() return U.char(0x80000000) end)
P("len strict", function() return U.len("\237\160\128"), U.len("\244\144\128\128") end)
P("len lax", function() return U.len("\237\160\128", 1, -1, true), U.len("\248\136\128\128\128", 1, -1, true) end)
P("len bounds", function() return U.len("abc", 5) end)
P("codepoint bounds", function() return U.codepoint("abc", 0) end)
P("codepoint lax", function() return U.codepoint("\253\191\191\191\191\191", 1, 1, true) end)
P("offset", function() return U.offset("a\226\130\172b", 2) end)
P("offset end", function() return U.offset("a\226\130\172b", -1), U.offset("a\226\130\172b", 0, 4) end)
P("offset bounds", function() return U.offset("abc", 1, 5) end)
P("codes cont start", function() return U.codes("\128a") end)
P("codes lax", function()
  local r = {}
  for p, c in U.codes("a\237\160\128", true) do r[#r + 1] = p .. ":" .. c end
  return table.concat(r, ",")
end)
P("codes strict", function() for p, c in U.codes("a\237\160\128") do end end)
