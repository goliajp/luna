-- a collection run with the C stack nearly full leaves finalizers for later;
-- the depths are counted back from the deepest pcall nesting that still
-- runs, which depends on how the file is run
local ran = {}
local function mk(tag) setmetatable({}, {__gc = function() ran[#ran + 1] = tag end}) end
local function fits(depth)
  if depth == 0 then return true end
  local ok, r = pcall(fits, depth - 1)
  return ok and r
end
local limit = 0
while fits(limit + 1) do limit = limit + 1 end
local function at(depth)
  if depth == 0 then
    mk(depth)
    collectgarbage()
    return #ran
  end
  local ok, r = pcall(at, depth - 1)
  return r
end
for k = 4, 0, -1 do
  ran = {}
  local n = at(limit - k)
  collectgarbage()
  print(k, (tostring(n):gsub("[^%s(]+:%d+: ", "")), #ran)
end
