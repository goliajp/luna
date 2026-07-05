-- v2.15 P2.5 (5.3): integer numeric-for preserves integer.
for i = 1, 3 do io.write(math.type(i), " ") end
print()
-- float coerces
for i = 1.0, 3.0 do io.write(math.type(i), " ") end
print()
