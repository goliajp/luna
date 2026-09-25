-- v3.1 string slice: utf8 in 5.3 (no lax flag, 5.3 decoder and wording)
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
P("charpattern", function() return U.charpattern == "[\0-\127\194-\244][\128-\191]*" end)
P("char max", function() return #U.char(0x10FFFF) end)
P("char over", function() return U.char(0x110000) end)
P("char float", function() return U.char(65.0) end)
P("len surrogate", function() return U.len("\237\160\128") end)
P("len over", function() return U.len("\244\144\128\128") end)
P("len 5 byte", function() return U.len("\248\136\128\128\128") end)
P("len lax ignored", function() return U.len("\244\144\128\128", 1, -1, true) end)
P("len bounds", function() return U.len("abc", 5) end)
P("len bounds 2", function() return U.len("abc", 1, 4) end)
P("codepoint bounds", function() return U.codepoint("abc", 0) end)
P("codepoint bounds 2", function() return U.codepoint("abc", 1, 4) end)
P("codepoint surrogate", function() return U.codepoint("\237\160\128") end)
P("offset bounds", function() return U.offset("abc", 1, 5) end)
P("offset", function() return U.offset("a\226\130\172b", 3), U.offset("a\226\130\172b", -1) end)
P("offset cont", function() return U.offset("a\226\130\172b", 1, 3) end)
P("codes", function()
  local r = {}
  for p, c in U.codes("a\226\130\172\237\160\128") do r[#r + 1] = p .. ":" .. c end
  return table.concat(r, ",")
end)
P("codes bad", function() for p, c in U.codes("a\128") do end end)
P("codes cont start", function()
  local r = {}
  for p, c in U.codes("\128a") do r[#r + 1] = p .. ":" .. c end
  return table.concat(r, ",")
end)
