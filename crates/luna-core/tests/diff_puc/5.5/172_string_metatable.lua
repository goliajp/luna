-- v2.13 CORPUS-IV: shared string metatable — method syntax on
-- string values routes through getmetatable("").__index == string.
local mt = getmetatable("")
print(type(mt), mt.__index == string)
print(("hello"):upper())
print(("a,b,c"):find(",", 1, true))
local s = "world"
print(s:len(), s:sub(2, 3))
print(getmetatable("x") == getmetatable("yy"))
