-- __name joined the type name in error messages in 5.3; 5.1/5.2 ignore it.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
local pre = "local o = setmetatable({}, {__name = 'MyType'}) "
e(pre .. "return o + 1")
e(pre .. "return o < o")
e(pre .. "return 'a' .. o")
e(pre .. "return o()")
