-- v2.13 CORPUS-IV: pcall multivalue passthrough + xpcall handler
-- args + handler that itself errors.
print(pcall(function(a, b) return a + b, a * b end, 3, 4))
local ok, r = xpcall(function() error("inner", 0) end, function(e) return "handled:" .. e end)
print(ok, r)
local ok2, r2 = xpcall(function() error("x", 0) end, function() error("handler_boom") end)
print(ok2, r2)
print(xpcall(function(v) return v * 2 end, print, 21))
