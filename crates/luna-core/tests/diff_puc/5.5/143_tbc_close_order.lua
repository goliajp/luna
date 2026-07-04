-- v2.12 CORPUS-III: to-be-closed variables (5.4+) — reverse
-- close order + __close fires on error unwinding under pcall.
local function closer(name)
  return setmetatable({}, { __close = function() print("close", name) end })
end
do
  local a <close> = closer("a")
  local b <close> = closer("b")
  print("body")
end
print("after block")
local ok = pcall(function()
  local c <close> = closer("c")
  error("boom")
end)
print("pcall ok", ok)
