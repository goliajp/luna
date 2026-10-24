-- a key set to nil and written back takes its own node again: PUC finds
-- the key there and writes the value without making a new key, so the
-- table neither grows nor moves anything
collectgarbage("stop")
local keys, t = {}, {}
for i = 1, 24 do
  keys[i] = (i % 3 == 0) and function() return i end or {}
  t[keys[i]] = i
end
local before = collectgarbage("count")
for r = 1, 6 do
  for i = 1, 24, 2 do t[keys[i]] = nil end
  for i = 1, 24, 2 do t[keys[i]] = i * r end
end
local grew = collectgarbage("count") - before
local n, sum = 0, 0
for k, v in pairs(t) do
  n = n + 1
  sum = sum + v
end
print(grew, n, sum)
