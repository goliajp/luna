local function inner(i) if i == 500 then error("hot " .. i) end return i end
local function mid(i) return inner(i) + 1 end
local s = 0
for i = 1, 1000 do s = s + mid(i) end
print(s)
