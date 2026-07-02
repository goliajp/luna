-- v2.11 CORPUS-II: __call chain.
local M = {}
M.__call = function(self, ...) return self.wrapped(...) end
local function wrap(f) return setmetatable({wrapped = f}, M) end
local sum = wrap(function(a, b) return a + b end)
print(sum(3, 4))
print(sum(10, 20))

-- nested
local double_sum = wrap(function(a, b) return sum(a, b) * 2 end)
print(double_sum(3, 4))
