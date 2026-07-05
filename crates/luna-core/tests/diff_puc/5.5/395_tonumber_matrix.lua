-- v2.14 CV.3: tonumber string forms + bases.
print(tonumber("0x1F"), tonumber("  10  "), tonumber("1e2"))
print(tonumber("z", 36), tonumber("11", 2), tonumber("ff", 16))
print(tonumber("0x", 16) == nil and tonumber("0x", 16) or tonumber("x", 36))
print(tonumber("10", 8), tonumber("9", 8))
print(tonumber(""), tonumber("abc"), tonumber("1.5.2"))
print(tonumber(true))
