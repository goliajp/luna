-- v2.14 CV.3: __newindex table target vs function trap.
local store = {}
local t = setmetatable({}, { __newindex = store })
t.a = 1
print(rawget(t, "a"), store.a)
local log = {}
local u = setmetatable({}, { __newindex = function(_, k, v) log[#log + 1] = k .. "=" .. v end })
u.x = 10
u.y = 20
rawset(u, "z", 30)
print(table.concat(log, ","), u.z)
