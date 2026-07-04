-- v2.14 HD 5.1 seed: setfenv/getfenv (removed in 5.2).
local function f() return x end
setfenv(f, { x = "sandboxed" })
print(f())
print(getfenv(f).x)
local g = function() return type(print) end
print(getfenv(g) == _G)
