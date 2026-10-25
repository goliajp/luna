-- a finalizer's error stops the collector among its finalizers; the full
-- collection asked for then has no sweep to end first, so it shrinks the
-- concatenation buffer once (checkSizes), not twice
local function grow()
  local s = string.rep("a", 300) .. string.rep("b", 300)
  return #s
end
local out = {}
for i = 1, 3 do
  grow()
  local p = newproxy(true)
  getmetatable(p).__gc = function() error("gc") end
  p = nil
  local ok = pcall(collectgarbage)
  local before = collectgarbage("count")
  collectgarbage()
  out[#out + 1] = tostring(ok) .. " " .. (collectgarbage("count") - before) * 1024
end
print(table.concat(out, "\n"))
