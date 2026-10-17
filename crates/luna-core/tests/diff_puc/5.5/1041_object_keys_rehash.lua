-- tables keyed by tables, functions and coroutines, grown, emptied and
-- refilled: what does not depend on the order of the keys
local keys = {}
for i = 1, 300 do
  keys[i] = (i % 3 == 0 and {}) or (i % 3 == 1 and function() return i end) or coroutine.create(function() end)
end
local t = {}
for i = 1, 300 do t[keys[i]] = i end
local n, sum = 0, 0
for k, v in pairs(t) do n = n + 1; sum = sum + v end
print(n, sum)
for i = 1, 300, 2 do t[keys[i]] = nil end
n, sum = 0, 0
for k, v in pairs(t) do n = n + 1; sum = sum + v end
print(n, sum)
for i = 1, 300, 2 do t[keys[i]] = i * 2 end
n, sum = 0, 0
for k, v in pairs(t) do n = n + 1; sum = sum + v end
print(n, sum)
