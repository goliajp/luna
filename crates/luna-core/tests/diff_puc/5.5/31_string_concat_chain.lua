-- v2.10 CORPUS: string concat right-assoc semantics.
print("a" .. "b" .. "c")
print(1 .. 2 .. 3)         -- 123 as string
-- concat with __concat metamethod
local M = {}
M.__concat = function(a, b) return "["..tostring(a)..","..tostring(b).."]" end
local x = setmetatable({name="X"}, M)
M.__tostring = function(t) return t.name end
print(x .. "y")
print("y" .. x)
print(x .. x)
