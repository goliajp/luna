-- v2.15 P2.5 (5.2): metatables basic.
local V = {}
V.__add = function(a, b) return setmetatable({v = a.v + b.v}, V) end
V.__tostring = function(v) return "<" .. v.v .. ">" end
local a = setmetatable({v = 3}, V)
local b = setmetatable({v = 4}, V)
print(tostring(a + b))
print((a + b).v)
