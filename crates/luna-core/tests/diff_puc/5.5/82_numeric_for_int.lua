-- v2.11 CORPUS-II: numeric-for stays integer when start/step are integer.
for i = 1, 5 do io.write(math.type(i), " ") end
print()   -- integer integer ...

for i = 1.0, 5.0 do io.write(math.type(i), " ") end
print()   -- float float ...

-- integer overflow guard in numeric-for (5.4+ semantics)
local n = 0
for i = math.maxinteger - 2, math.maxinteger do n = n + 1 end
print(n)
