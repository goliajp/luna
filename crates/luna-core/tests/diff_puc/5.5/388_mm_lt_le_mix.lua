-- v2.14 CV.3: __lt/__le dispatch on both operands.
local mt = {
  __lt = function(x, y) return x.v < y.v end,
  __le = function(x, y) return x.v <= y.v end,
}
local a = setmetatable({ v = 1 }, mt)
local b = setmetatable({ v = 2 }, mt)
print(a < b, b < a, a <= b, b <= a)
print(a > b, a >= b)
