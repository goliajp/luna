-- v2.13 CORPUS-IV: table.pack/unpack + n field + ranges.
local p = table.pack(1, nil, "three")
print(p.n, p[1], p[2], p[3])
print(table.unpack({ "a", "b", "c" }))
print(table.unpack({ "a", "b", "c" }, 2))
print(table.unpack({ "a", "b", "c" }, 2, 3))
print(table.unpack({ "a" }, 1, 3))
print(table.unpack({}, 1, 0))
print(select("#", table.unpack({}, 1, 0)))
