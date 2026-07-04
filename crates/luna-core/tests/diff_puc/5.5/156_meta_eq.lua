-- v2.13 CORPUS-IV: __eq — consulted only when both operands are
-- tables (or both userdata); primitive comparisons never consult.
local mt = { __eq = function(a, b) return true end }
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
local c = {}
print(a == b)      -- true via __eq
print(a == c)      -- true: 5.3+ consults if either has __eq
print(a == a)      -- true: identity short-circuits
print(a == 1, a == "x", a == nil)  -- false: type mismatch, no consult
print(1 == 1.0)    -- true: numeric, no metamethod
