-- v2.13 CORPUS-IV: tonumber with explicit base 2..36.
print(tonumber("1010", 2), tonumber("777", 8))
print(tonumber("ff", 16), tonumber("FF", 16))
print(tonumber("z", 36), tonumber("10", 36))
print(tonumber("102", 2))
print(tonumber("  11  ", 2))
print((pcall(tonumber, "10", 1)))
print((pcall(tonumber, "10", 37)))
print(math.type(tonumber("ff", 16)))
