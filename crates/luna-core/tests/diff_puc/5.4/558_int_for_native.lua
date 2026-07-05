-- v2.15 P2.5: integer-native numeric-for stays Int.
for i = 1, 5 do print(math.type(i), i) end
-- but float coerces
for i = 1.0, 3.0 do print(math.type(i), i) end
