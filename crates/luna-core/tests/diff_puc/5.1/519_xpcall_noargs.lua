-- v2.14 HD 5.1 seed: xpcall does NOT forward extra arguments to
-- the called function (5.2+ added that).
local ok, v = xpcall(function(a) return a end, function(e) return e end, 42)
print(ok, v)
local ok2, v2 = xpcall(function() return "noargs" end, print)
print(ok2, v2)
