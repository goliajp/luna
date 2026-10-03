local function c(t) return t.x.y end
local function b(t) return c(t) + 1 end
local s = 0
for i = 1, 1000 do s = s + b(i == 900 and {} or {x = {y = i}}) end
