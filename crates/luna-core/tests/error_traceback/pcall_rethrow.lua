local function inner() error("inner") end
local ok, e = pcall(inner)
local function outer() error(e, 0) end
outer()
