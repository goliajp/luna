-- v2.11 CORPUS-II: chained __add.
local N = {}
N.__index = N
-- Operands may be plain numbers (numbers can't be indexed) —
-- unwrap by type, not by indexing.
local function val(x)
  if type(x) == "table" then return x.v end
  return x
end
N.__add = function(a, b)
  return setmetatable({v = val(a) + val(b)}, N)
end
N.__tostring = function(x) return tostring(x.v) end
local function n(v) return setmetatable({v=v}, N) end
print(tostring(n(1) + n(2)))
print(tostring(n(1) + n(2) + n(3) + n(4)))
print(tostring(n(10) + 5))     -- 10+5=15 (b=5, .v is nil, falls to b)
print(tostring(3 + n(7)))
