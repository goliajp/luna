-- v3.1 debug slice: getupvalue / setupvalue on Lua and C functions, and
-- their argument checks.
local u1, u2 = 1, 2
local function f() return u1 + u2 end
print(debug.getupvalue(f, 1), debug.getupvalue(f, 3))
print(select("#", debug.getupvalue(f, 3)))
print(debug.setupvalue(f, 1, 10), u1)
print(select("#", debug.setupvalue(f, 7, 10)))
local it = string.gmatch("a", "a")
print(select("#", debug.getupvalue(it, 1)), select("#", debug.getupvalue(math.abs, 1)))
print(pcall(debug.getupvalue, 1, 1))
print(pcall(debug.getupvalue, f, "x"))
print(pcall(debug.getupvalue))
print(pcall(debug.setupvalue, f, 1))
