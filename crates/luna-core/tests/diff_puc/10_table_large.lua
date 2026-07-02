-- v2.10 CORPUS: table.concat + insert/remove on larger arrays.
local t = {}
for i = 1, 20 do t[i] = i end
print(table.concat(t, ","))
print(table.concat(t, "|", 5, 10))
table.remove(t, 1)
print(t[1], t[19], #t)
table.insert(t, 1, 999)
print(t[1], t[2], #t)
local u = {"x", "y", "z"}
print(table.concat(u))          -- no sep
print(table.concat(u, "-"))
print(table.concat({}, ","))    -- empty
