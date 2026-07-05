-- v2.14 HD 5.2 seed: _ENV arrives; load takes (chunk, name, mode, env).
local f = load("return x", "c", "t", { x = 7 })
print(f())
print(type(_ENV))
local function g() return _ENV == _G end
print(g())
print(setfenv == nil, loadstring == nil or type(loadstring))
