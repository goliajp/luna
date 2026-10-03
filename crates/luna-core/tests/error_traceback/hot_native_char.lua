local function enc(i) return string.char(i) end
local t = {}
for i = 1, 300 do t[#t + 1] = enc(i) end
