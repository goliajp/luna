-- 5.1's collectgarbage("step") sets the threshold "stop" had put out of
-- reach, so the collector runs on its own again after it: the garbage the
-- loop makes is collected as it goes
collectgarbage()
collectgarbage("stop")
local base = collectgarbage("count")
collectgarbage("step", 0)
local t = {}
for i = 1, 3000 do t[i % 50] = {i} end
print(string.format("%.0f", (collectgarbage("count") - base) * 1024))
