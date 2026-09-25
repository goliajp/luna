-- A multiple assignment evaluates every expression first and then
-- stores from the last target to the first, which __newindex and a
-- repeated target can see.
local log = {}
local mt = {__newindex = function(t, k, v) log[#log + 1] = k .. "=" .. tostring(v) rawset(t, k, v) end}
local x = setmetatable({}, mt)
x.a, x.b, x.c = 1, 2, 3
print(table.concat(log, " "))
log = {}
x.d, x.e = (function() return 4, 5, 6 end)()
print(table.concat(log, " "))
log = {}
x.f, x.g, x.h = 7
print(table.concat(log, " "))
local a
a, a = 1, 2
print(a)
local t = {}
t.k, t.k = "first", "second"
print(t.k)
local i = 1
local arr = {}
i, arr[i] = i + 1, 20
print(i, arr[1], arr[2])
