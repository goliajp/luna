-- 5.1 ccs=UTF-8 streams on bytes that are not UTF-8 and on lone surrogates.
-- Run with a case number; file names are under the global DIR when it is set.
local D = DIR or ""
local function P(n) return D .. n end
local function esc(s)
  if s == nil then return "nil" end
  if type(s) ~= "string" then return tostring(s) end
  return (s:gsub("[%c\128-\255\\]", function(c) return string.format("\\%d", c:byte()) end))
end
local function r(...) local t = {} for i = 1, select("#", ...) do t[#t+1] = esc((select(i, ...))) end return table.concat(t, ",") end
local function put(name, s) local f = assert(io.open(name, "wb")); f:write(s); f:close() end
local function raw(name) local f = assert(io.open(name, "rb")); local s = f:read("*a"); f:close(); return s end
local FILES = {
  cont = "a\128b", trunc2 = "a\195b", trunc2end = "ab\195", trunc3 = "a\226\130b", trunc3end = "ab\226\130", trunc4end = "ab\240\159\152",
  overlong = "a\192\128b", surrogate = "a\237\160\128b", ff = "a\255b", beyond = "a\244\144\128\128b", leadlead = "a\195\195\169b",
  contafter = "a\195\169\128b", many = "\128\128\128\128", nul = "a\0b", long = string.rep("\195\169", 3000) .. "\195",
}
local KEYS = { "cont", "trunc2", "trunc2end", "trunc3", "trunc3end", "trunc4end", "overlong", "surrogate", "ff", "beyond", "leadlead", "contafter", "many", "nul", "long" }
local cases = {}
for _, k in ipairs(KEYS) do
  for _, how in ipairs({ "*a", "2", "4" }) do
    cases[#cases+1] = function()
      put(P("i.txt"), FILES[k])
      local f = io.open(P("i.txt"), "r, ccs=UTF-8")
      local t = {}
      local fmt = tonumber(how) or how
      for i = 1, 4 do t[#t+1] = r(f:read(fmt)) .. "@" .. r(f:seek("cur")) end
      f:close()
      return table.concat({ "read", k, how, table.concat(t, "|") }, " ")
    end
  end
end
local WRITES = { high = "\0\216", highA = "\0\216a\0", low = "\0\220", lowhigh = "\0\220\0\216", pair = "\61\216\0\222", fffd = "\253\255", nl = "\13\0\10\0", mixed = "a\0\0\216b\0" }
for _, k in ipairs({ "high", "highA", "low", "lowhigh", "pair", "fffd", "nl", "mixed" }) do
  cases[#cases+1] = function()
    os.remove(P("o.txt"))
    local f = io.open(P("o.txt"), "w, ccs=UTF-8")
    local w = r(f:write(WRITES[k])); local s = r(f:seek("cur")); f:close()
    return table.concat({ "write", k, w, s, esc(raw(P("o.txt"))) }, " ")
  end
end
if arg[1] == "count" then print(#cases) else print(arg[1], cases[tonumber(arg[1])]()) end
