-- v2.13 CORPUS-IV: __index fires for MISSING keys; a key set
-- then removed (=nil) is missing again.
local mt = { __index = function(_, k) return "idx:" .. tostring(k) end }
local t = setmetatable({}, mt)
print(t.a)
t.a = "real"
print(t.a)
t.a = nil
print(t.a)
t.b = false
print(t.b)
