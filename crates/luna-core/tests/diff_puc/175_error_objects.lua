-- v2.13 CORPUS-IV: error objects — non-string values pass
-- through pcall by identity; level-0 strings unprefixed.
local sentinel = { tag = "err_table" }
local ok, e = pcall(function() error(sentinel) end)
print(ok, e == sentinel, e.tag)
local ok2, e2 = pcall(function() error(42) end)
print(ok2, e2, math.type(e2))
local ok3, e3 = pcall(function() error() end)
print(ok3, e3)
local ok4, e4 = pcall(function() error("plain", 0) end)
print(ok4, e4)
-- error(nil) → nil object
local ok5, e5 = pcall(error, nil)
print(ok5, e5)
