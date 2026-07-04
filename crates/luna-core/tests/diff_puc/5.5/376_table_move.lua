-- v2.14 CV.3: table.move within/between tables, overlapping.
local a = { 1, 2, 3, 4, 5 }
table.move(a, 2, 4, 1)
print(table.concat(a, ","))
local b = table.move({ 10, 20, 30 }, 1, 3, 2, {})
print(b[1], b[2], b[3], b[4])
local c = { 1, 2, 3, 4 }
table.move(c, 1, 3, 2)
print(table.concat(c, ","))
