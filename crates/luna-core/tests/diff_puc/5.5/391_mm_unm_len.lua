-- v2.14 CV.3: __unm and __len together.
local t = setmetatable({ 1, 2, 3 }, {
  __unm = function(self) return "negated" end,
  __len = function() return 99 end,
})
print(-t, #t)
print(rawlen(t))
