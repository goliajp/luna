-- a handler that fails at once, with no Lua frame of its own
print(xpcall(function() error("a") end, error))
print(select('#', xpcall(function() error("a") end, error)))
-- a native handler failing on a bad argument
print(xpcall(function() error("a") end, string.rep))
-- a handler that recurses into a failing native
local n = 0
local function h(m) n = n + 1; return string.rep(m) end
print(xpcall(function() error("a") end, h), n > 0)
-- a handler that fails only the first few times
local k = 0
print(xpcall(function() error("a") end, function(m) k = k + 1; if k < 5 then error(m, 0) end; return "done " .. k end))
-- the error table, not a string
print(pcall(xpcall, function() error({}) end, error))
