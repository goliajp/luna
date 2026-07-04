-- v2.11 CORPUS-II: table.pack round-trip.
local t = table.pack(1, 2, 3, 4, 5)
print(t.n)
for i = 1, t.n do io.write(t[i], " ") end
print()

-- with nil in middle
local u = table.pack(1, nil, 3)
print(u.n)   -- 3
print(u[1], u[2], u[3])
