-- v2.14 CV.3: numeric for with negative/large steps.
for i = 10, 1, -3 do io.write(i, " ") end
print()
for i = 1, 10, 4 do io.write(i, " ") end
print()
local n = 0
for i = math.mininteger, math.mininteger + 2 do n = n + 1 end
print(n)
for i = 1.0, 2.5, 0.5 do io.write(i, " ") end
print()
