-- v2.14 CV.3: sort with comparator + reverse + already-sorted.
local t = { 5, 2, 8, 1, 9, 3 }
table.sort(t)
print(table.concat(t, ","))
table.sort(t, function(x, y) return x > y end)
print(table.concat(t, ","))
table.sort(t, function(x, y) return x > y end)
print(table.concat(t, ","))
