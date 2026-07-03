-- v2.13 CORPUS-IV: nested pcall — inner error caught inner,
-- rethrow crosses one level, error objects pass by identity.
local obj = { tag = "obj" }
local ok_outer, v = pcall(function()
  local ok_inner, e = pcall(function() error(obj) end)
  return ok_inner, e == obj
end)
print(ok_outer, v)
local ok2, e2 = pcall(function()
  local ok3 = pcall(error, "eaten")
  error("rethrown", 0)
end)
print(ok2, e2)
print(pcall(pcall, error, "double"))
