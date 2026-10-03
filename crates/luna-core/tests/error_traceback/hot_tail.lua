local function a(i) if i == 900 then error("t") end return i end
local function b(i) return a(i) end
local s = 0
for i = 1, 1000 do s = s + b(i) end
