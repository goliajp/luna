-- rule: lgc.traversetable.3
local function cnt(t) local n = 0 for _ in pairs(t) do n = n + 1 end return n end
local a = setmetatable({}, {__mode = "\0k"})
a[{}] = 1
local b = setmetatable({}, {__mode = string.rep("x", 40) .. "k"})
b[{}] = 1
local c = setmetatable({}, {__mode = string.rep("x", 39) .. "k"})
c[{}] = 1
collectgarbage(); collectgarbage()
print("nul-k", cnt(a), "long41", cnt(b), "short40", cnt(c))
