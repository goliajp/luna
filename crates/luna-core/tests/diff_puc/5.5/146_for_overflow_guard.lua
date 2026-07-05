-- v2.12 CORPUS-III: integer numeric-for near maxinteger must
-- terminate without wrapping (5.4+ overflow guard).
local n = 0
for i = math.maxinteger - 2, math.maxinteger do n = n + 1 end
print(n)
local m = 0
for i = math.mininteger, math.mininteger + 2, -1 do m = m + 1 end
print(m)
local k = 0
for i = math.mininteger + 2, math.mininteger, -1 do k = k + 1 end
print(k)
