local function wrap(i, n) return i % n end
local s = 0
for i = 1, 300 do s = s + wrap(i, 300 - i) end
error("no error from a float modulo")
