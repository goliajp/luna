-- closures built twice from one function expression: 5.2 / 5.3 reuse the
-- prototype's last closure when every upvalue is the same, 5.4 removed that
-- cache and always builds a new one
local outer = 1
local function mk() return function() return outer end end
local a, b = mk(), mk()
print(a == b, rawequal(a, b))
local t = {}
t[a] = 1
t[b] = 2
local n = 0
for _ in pairs(t) do n = n + 1 end
print(n)
local fs = {}
for i = 1, 3 do fs[i] = function() return outer + 1 end end
print(fs[1] == fs[2], fs[2] == fs[3])
local function k() return function() end end
print(k() == k())
local gs = {}
for i = 1, 2 do gs[i] = function() return i end end
print(gs[1] == gs[2], gs[1](), gs[2]())
