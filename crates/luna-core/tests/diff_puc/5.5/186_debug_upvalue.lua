-- v2.13 CORPUS-IV: debug.getupvalue/setupvalue name + value.
local x, y = "ex", "why"
local function f() return x .. y end
local n1, v1 = debug.getupvalue(f, 1)
local n2, v2 = debug.getupvalue(f, 2)
print(n1, v1, n2, v2)
print(debug.getupvalue(f, 3))
debug.setupvalue(f, 1, "EX")
print(f(), x)
