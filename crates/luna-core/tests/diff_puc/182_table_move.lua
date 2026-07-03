-- v2.13 CORPUS-IV: table.move — basic, overlapping (both
-- directions), and cross-table.
local t = { 1, 2, 3, 4, 5 }
table.move(t, 1, 3, 3)
print(table.concat(t, ","))
local u = { 1, 2, 3, 4, 5 }
table.move(u, 3, 5, 1)
print(table.concat(u, ","))
local src, dst = { "a", "b", "c" }, { "x", "y", "z", "w" }
local r = table.move(src, 1, 3, 2, dst)
print(r == dst, table.concat(dst, ","))
print(table.concat(table.move({ 9 }, 1, 0, 1, { "keep" }), ","))
