local function c(t) return t.x * 2 end
local function b(t) local r = c(t); return r + 1 end
local s = 0
for i = 1, 1000 do s = s + b(i == 900 and {x = "z"} or {x = i}) end
