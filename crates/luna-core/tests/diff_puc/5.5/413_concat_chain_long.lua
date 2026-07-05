-- v2.15 P2.4: long string concat chain.
local s = "a" .. "b" .. "c" .. "d" .. "e" .. "f" .. "g" .. "h" .. "i" .. "j"
print(s)
print(#s)
local n = 0
for i = 1, 100 do n = n .. "" .. "" end
print(#tostring(n))
