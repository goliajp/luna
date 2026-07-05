-- v2.10 CORPUS: __lt / __le metamethods.
local P = {}
P.__index = P
P.__lt = function(a, b) return a.v < b.v end
P.__le = function(a, b) return a.v <= b.v end
P.__eq = function(a, b) return a.v == b.v end
local function p(v) return setmetatable({v=v}, P) end
print(p(1) < p(2), p(2) < p(1))
print(p(3) <= p(3), p(3) <= p(2))
print(p(5) == p(5), p(5) == p(6))
