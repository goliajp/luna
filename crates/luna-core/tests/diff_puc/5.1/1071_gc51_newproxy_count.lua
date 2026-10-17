-- what newproxy counts: a proxy, the metatable it makes, and the growth
-- of the weak table that remembers those metatables
collectgarbage()
collectgarbage()
collectgarbage("stop")
local before = collectgarbage("count")
local keep = {}
for i = 1, 20 do keep[i] = newproxy(true) end
keep[21] = newproxy(keep[1])
keep[22] = newproxy(false)
print(string.format("%.0f", (collectgarbage("count") - before) * 1024))
keep = nil
collectgarbage()
print(string.format("%.0f", (collectgarbage("count") - before) * 1024))
