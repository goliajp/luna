-- v2.13 CORPUS-IV: multiple assignment — RHS fully evaluated
-- before any store; extra RHS dropped; missing padded nil.
local a, b = 1, 2
a, b = b, a
print(a, b)
local t = { 10, 20 }
t[1], t[2] = t[2], t[1]
print(t[1], t[2])
local x, y, z = 1
print(x, y, z)
local p, q = 1, 2, 3
print(p, q)
local i = 1
local arr = { "one", "two" }
i, arr[i] = 2, "changed"
print(i, arr[1], arr[2])
