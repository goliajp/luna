-- v2.13 CORPUS-IV: load with explicit env (4th arg, 5.2+).
local env = { x = 10, print = print }
local f = load("return x * 2", "chunk", "t", env)
print(f())
local g = load("y = 99; return y", "chunk", "t", env)
print(g(), env.y, y)
local h = load("return print ~= nil", "chunk", "t", {})
print(h())
