-- v2.13 CORPUS-IV: __unm and __len metamethods.
local mt = {
  __unm = function(x) return "neg:" .. x.v end,
  __len = function(x) return 42 end,
}
local o = setmetatable({ v = "k" }, mt)
print(-o)
print(#o)
local t = setmetatable({ 1, 2, 3 }, { __len = function() return 99 end })
print(#t)
print(rawlen(t))
