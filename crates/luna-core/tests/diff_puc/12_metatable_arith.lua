-- v2.10 CORPUS: arithmetic metamethods.
local V = {}
V.__index = V
V.__add = function(a, b) return setmetatable({x=a.x+b.x, y=a.y+b.y}, V) end
V.__sub = function(a, b) return setmetatable({x=a.x-b.x, y=a.y-b.y}, V) end
V.__mul = function(a, k) return setmetatable({x=a.x*k, y=a.y*k}, V) end
V.__tostring = function(v) return string.format("(%d,%d)", v.x, v.y) end
V.__eq = function(a, b) return a.x == b.x and a.y == b.y end
local function v(x, y) return setmetatable({x=x, y=y}, V) end
local p = v(3, 4)
local q = v(1, 2)
print(tostring(p + q))
print(tostring(p - q))
print(tostring(p * 2))
print(p == v(3, 4))
print(p == q)
