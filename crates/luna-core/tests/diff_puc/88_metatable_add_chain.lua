-- v2.11 CORPUS-II: chained __add.
local N = {}
N.__index = N
N.__add = function(a, b)
  return setmetatable({v = (a.v or a) + (b.v or b)}, N)
end
N.__tostring = function(x) return tostring(x.v) end
local function n(v) return setmetatable({v=v}, N) end
print(tostring(n(1) + n(2)))
print(tostring(n(1) + n(2) + n(3) + n(4)))
print(tostring(n(10) + 5))     -- 10+5=15 (b=5, .v is nil, falls to b)
print(tostring(3 + n(7)))
