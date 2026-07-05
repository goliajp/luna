-- v2.14 HD 5.1 seed: global unpack (moved to table.unpack in 5.2).
print(unpack({ 1, 2, 3 }))
print(unpack({ "a", "b", "c" }, 2))
print(unpack({ "x" }, 1, 3))
print(type(unpack))
