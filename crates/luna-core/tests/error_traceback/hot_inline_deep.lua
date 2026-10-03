local function d3(i) if i == 280 then error("d3 " .. i) end return i end
local function d2(i) local r = d3(i); return r end
local function d1(i) local r = d2(i); return r + 1 end
local s = 0
for i = 1, 300 do s = s + d1(i) end
