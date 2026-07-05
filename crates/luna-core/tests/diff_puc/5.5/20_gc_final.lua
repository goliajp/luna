-- v2.10 CORPUS: collectgarbage + weak tables.
local weak = setmetatable({}, {__mode = "v"})
local strong = {}
for i = 1, 3 do
  local t = {i = i}
  strong[i] = t
  weak[i] = t
end
collectgarbage("collect")
print(#strong, weak[1].i, weak[2].i, weak[3].i)  -- 3 1 2 3

-- clear strong refs
for i = 1, 3 do strong[i] = nil end
collectgarbage("collect")
-- weak entries should be gone
local n = 0
for _ in pairs(weak) do n = n + 1 end
print(n)  -- 0
