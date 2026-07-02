-- v2.13 CORPUS-IV: rawget/rawset/rawequal/rawlen bypass all
-- metamethods.
local mt = {
  __index = function() return "via_index" end,
  __newindex = function() error("newindex must not fire") end,
  __eq = function() return true end,
  __len = function() return 999 end,
}
local a = setmetatable({ 10, 20 }, mt)
local b = setmetatable({}, mt)
print(a.missing, rawget(a, "missing"))
rawset(a, "k", "v")
print(rawget(a, "k"))
print(a == b, rawequal(a, b), rawequal(a, a))
print(#a, rawlen(a))
