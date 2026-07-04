-- v2.11 CORPUS-II: table.move self-overlapping.
local t = {1, 2, 3, 4, 5}
-- shift right (overlap)
table.move(t, 1, 4, 2)
print(t[1], t[2], t[3], t[4], t[5])
