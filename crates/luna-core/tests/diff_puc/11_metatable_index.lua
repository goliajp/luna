-- v2.10 CORPUS: __index chains.
local a = {a1 = "A1"}
local b = setmetatable({b1 = "B1"}, {__index = a})
local c = setmetatable({c1 = "C1"}, {__index = b})
print(c.c1, c.b1, c.a1)
print(c.nope)  -- nil
-- __index as function
local d = setmetatable({}, {__index = function(t, k) return "resolved:" .. k end})
print(d.foo)
print(d.bar)
-- __newindex
local written = {}
local e = setmetatable({}, {__newindex = function(t, k, v) written[k] = v end})
e.x = 10
e.y = 20
print(written.x, written.y)
