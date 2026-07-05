-- v2.14 HD 5.2 seed: unpack lives at table.unpack.
print(table.unpack({ 1, 2, 3 }))
print(table.unpack({ "a", "b", "c" }, 2, 3))
print(type(table.pack), table.pack(1, nil, 3).n)
