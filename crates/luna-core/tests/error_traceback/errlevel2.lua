local function f() error("x", 2) end
local function g() f() end
g()
