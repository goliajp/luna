-- v2.14 CV.3: insert/remove edge positions.
local t = { "a", "b" }
table.insert(t, "c")
table.insert(t, 1, "z")
print(table.concat(t, ","))
print(table.remove(t, 1))
print(table.remove(t))
print(table.concat(t, ","))
print(table.remove({}))
