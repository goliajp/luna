-- v3.1 string slice: 5.3 arithmetic on strings is float arithmetic (lvm.c
-- takes the integer path only for two integers); bitwise converts exactly.
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
local a, b, c = "10", "3", "9007199254740993"
P("add", function() return a + 1, math.type(a + 1) end)
P("mul", function() return a * b, math.type(a * b) end)
P("idiv", function() return a // b, a % b end)
P("idiv zero", function() return a // "0" end)
P("unm", function() return -a, -"0x10" end)
P("pow", function() return b ^ 2 end)
P("band", function() return a & 6, c & -1, ~c end)
P("band float", function() local x = "2.5" return x & 1 end)
P("bnot float", function() local x = "2.5" return ~x end)
P("band bad", function() local x = "abc" return x | 1 end)
P("for strings", function()
  local r = {}
  for i = "1", 3 do r[#r + 1] = i end
  return table.concat(r, ",")
end)
P("hot", function()
  local s = 0
  for i = 1, 300 do s = s + ("2" * i) end
  return s, math.type(s)
end)
