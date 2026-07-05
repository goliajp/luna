-- v2.15 P2.5: 5.4 overflow guard on numeric-for.
local n = 0
for i = math.maxinteger - 3, math.maxinteger do n = n + 1 end
print(n)   -- 4 (not infinite)

-- reverse
n = 0
for i = math.mininteger + 2, math.mininteger, -1 do n = n + 1 end
print(n)
