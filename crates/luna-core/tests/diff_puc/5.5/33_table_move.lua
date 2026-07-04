-- v2.10 CORPUS: table.move.
local src = {1, 2, 3, 4, 5}
local dst = {}
table.move(src, 1, #src, 1, dst)
print(#dst, dst[1], dst[5])

-- move within same table
local t = {10, 20, 30, 40, 50}
table.move(t, 2, 4, 1)
print(t[1], t[2], t[3], t[4], t[5])
