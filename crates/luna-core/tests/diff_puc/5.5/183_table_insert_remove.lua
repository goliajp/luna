-- v2.13 CORPUS-IV: table.insert/remove edge — middle insert,
-- remove returns the value, remove from empty, pos bounds error.
local t = { "a", "c" }
table.insert(t, 2, "b")
print(table.concat(t, ","))
print(table.remove(t, 1), table.concat(t, ","))
print(table.remove(t))
print(table.remove(t))
print(table.remove({}))
print((pcall(table.insert, { 1 }, 5, "x")))
print((pcall(table.insert, { 1 }, 1, "y")))
