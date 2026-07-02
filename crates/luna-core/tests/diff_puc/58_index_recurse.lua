-- v2.11 CORPUS-II: __index chain depth.
local function chain(depth)
  local prev = {}
  for i = 1, depth do
    prev = setmetatable({}, {__index = prev})
  end
  prev.found = "yes"
  return prev
end
local top = setmetatable({}, {__index = chain(20)})
print(top.found)   -- "yes"
print(top.other)   -- nil
