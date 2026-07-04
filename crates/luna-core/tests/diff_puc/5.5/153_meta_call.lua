-- v2.13 CORPUS-IV: __call — callable tables, args forwarding,
-- multiple returns.
local o = setmetatable({}, {
  __call = function(self, a, b)
    return a + b, a * b
  end,
})
print(o(3, 4))
print(type(o))
local callable_field = { f = o }
print(callable_field.f(2, 5))
local nested = setmetatable({}, { __call = function(_, x) return o(x, 10) end })
print(nested(7))
