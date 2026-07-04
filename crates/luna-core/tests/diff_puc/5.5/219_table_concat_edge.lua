-- v2.13 CORPUS-IV: table.concat sep/i/j + invalid element error.
local t = { "a", "b", "c", "d" }
print(table.concat(t))
print(table.concat(t, "+"))
print(table.concat(t, ",", 2, 3))
print(table.concat(t, ",", 3, 2) == "")
print(table.concat({ 1, 2, 3 }, "-"))
print((pcall(table.concat, { {} })))
print((pcall(table.concat, { "a", true })))
