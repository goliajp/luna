local function f(i) if i == 900 then return string.rep("x", -1, {}) end return #string.rep("x", i % 3) end
local s = 0
for i = 1, 1000 do s = s + f(i) end
