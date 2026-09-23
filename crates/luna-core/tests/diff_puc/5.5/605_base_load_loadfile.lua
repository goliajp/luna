-- load (and 5.1 loadstring): 5.1 load takes only a reader; 5.2+ also takes
-- a string or number, reads name and mode with luaL_optstring, checks the
-- mode against the chunk kind, and ignores env for upvalue-less functions.
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
-- 5.2's load and loadstring are one function with two global names; which
-- one an error names depends on the run's hash seed.
if _VERSION == "Lua 5.2" then
  local strip = clean
  clean = function(s) return (strip(s):gsub("'loadstring'", "'load'")) end
end
local function ld(label, ...)
  local ok, f, e = pcall(load, ...)
  print(label, ok, type(f), e and clean(e) or "")
end
ld("string", "return 1")
ld("number", 42)
ld("nil", nil)
ld("table", {})
ld("callable", setmetatable({}, {__call = function() end}))
ld("name table", "x(", {})
ld("name number", "x(", 42)
ld("mode x", "return 1", "c", "x")
ld("mode B", "return 1", "c", "B")
ld("mode number", "return 1", "c", 1)
ld("mode table", "return 1", "c", {})
local n = 0
ld("reader number", function() n = n + 1 if n == 1 then return "return " elseif n == 2 then return 5 end end)
ld("reader table", function() return {} end)
local ls = rawget(_G, "loadstring")
if ls then
  local ok, f, e = pcall(ls, "x(", "=named")
  print("loadstring", ok, type(f), e and clean(e) or "")
  print("loadstring number", pcall(ls, 42))
  print("loadstring fn", (pcall(ls, function() end)))
end
local lf = rawget(_G, "loadfile")
print("loadfile missing", select(2, lf("/nonexistent/luna-fixture.lua")))
print("dofile missing", select(2, pcall(dofile, "/nonexistent/luna-fixture.lua")))
print("loadfile table", clean(select(2, pcall(lf, {}))))
