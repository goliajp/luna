-- v2.14 CV.3: __call receives self + args, multiret.
local t = setmetatable({ tag = "callable" }, {
  __call = function(self, a, b) return self.tag, a + b, "extra" end,
})
print(t(3, 4))
local nested = setmetatable({}, { __call = t })
local ok, e = pcall(nested)
print(ok, e:match("attempt to perform arithmetic") ~= nil)
