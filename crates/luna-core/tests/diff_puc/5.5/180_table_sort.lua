-- v2.13 CORPUS-IV: table.sort default + custom comparator.
local t = { 5, 2, 8, 1, 9, 3 }
table.sort(t)
print(table.concat(t, ","))
table.sort(t, function(a, b) return a > b end)
print(table.concat(t, ","))
local words = { "banana", "Apple", "cherry" }
table.sort(words)
print(table.concat(words, ","))
table.sort(words, function(a, b) return a:lower() < b:lower() end)
print(table.concat(words, ","))
