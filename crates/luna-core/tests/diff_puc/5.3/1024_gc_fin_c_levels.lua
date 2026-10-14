-- a collection run with the C stack nearly full leaves finalizers for later
local ran = {}
local function mk(tag) setmetatable({}, {__gc = function() ran[#ran + 1] = tag end}) end
local function at(depth)
  if depth == 0 then
    mk(depth)
    collectgarbage()
    return #ran
  end
  local ok, r = pcall(at, depth - 1)
  return r
end
for _, d in ipairs{195, 196, 197} do
  ran = {}
  local n = at(d)
  collectgarbage()
  print(d, n, #ran)
end
