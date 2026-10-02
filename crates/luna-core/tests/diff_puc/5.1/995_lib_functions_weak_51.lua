-- 5.1 library functions are collectable objects: a weak table drops one
-- that nothing else holds
local t = setmetatable({}, {__mode = "v"})
local k = setmetatable({}, {__mode = "k"})
t[1], t[2] = string.rep, math.floor
k[string.rep], k[math.floor] = true, true
string.rep, math.floor = nil, nil
collectgarbage()
collectgarbage()
local n = 0
for _ in pairs(k) do n = n + 1 end
print(type(t[1]), type(t[2]), n)
print(t[1] and t[1]("ab", 2), t[2] and t[2](2.5))
