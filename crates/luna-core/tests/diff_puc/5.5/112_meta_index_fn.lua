-- v2.12 CORPUS-III: __index function priority.
-- Present key beats __index; missing key calls __index.
local m = setmetatable({a = "raw"}, {__index = function(t, k) return "meta:" .. k end})
print(m.a, m.b, m.c)     -- raw meta:b meta:c

-- rawget bypasses
print(rawget(m, "a"), rawget(m, "b"))

-- rawset bypasses too
local m2 = setmetatable({}, {__newindex = function() error("nope") end})
rawset(m2, "x", 42)
print(m2.x)
