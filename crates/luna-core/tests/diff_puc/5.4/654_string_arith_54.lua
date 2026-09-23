-- v3.1 string slice: 5.4 string arithmetic goes through the string
-- metatable (lstrlib stringmetamethods); bitwise operators do not convert.
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
local smt = getmetatable("")
local keys = {}
for k in pairs(smt) do keys[#keys + 1] = k end
table.sort(keys)
print(table.concat(keys, " "))
local o = setmetatable({}, {__add = function() return "mt" end})
local a = "10"
P("add", function() return a + 1, math.type(a + 1), a + 0.5 end)
P("idiv", function() return a // "3", a % "3", -a end)
P("idiv zero", function() return a // "0" end)
P("mod zero", function() return a % "0" end)
P("bad", function() return a + "x" end)
P("bad unm", function() return -"x" end)
P("bad table", function() return a + {} end)
P("second mm", function() return a + o, o + a end)
P("direct", function() return smt.__add("1", "2"), smt.__unm("3"), smt.__add("4") end)
P("direct bad", function() return smt.__add({}, "2") end)
P("direct none", function() return smt.__mul() end)
P("band", function() return a & 1 end)
P("bnot", function() return ~a end)
P("removed", function()
  local old = smt.__add
  smt.__add = nil
  local ok, e = pcall(function() return a + 1 end)
  smt.__add = old
  return ok, e
end)
P("hot", function()
  local s = 0
  for i = 1, 300 do s = s + ("2" * i) end
  return s, math.type(s)
end)
P("hot late error", function()
  local t = {}
  for i = 1, 300 do t[i] = tostring(i) end
  t[250] = "x"
  local s = 0
  for i = 1, 300 do s = s + t[i] * 2 end
  return s
end)
P("for strings", function()
  local r = {}
  for i = "1", 3 do r[#r + 1] = i end
  return table.concat(r, ",")
end)
