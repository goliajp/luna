-- v2.14 CV.3: lua_Integer width codes.
print(#string.pack("<j", 1), #string.pack("<J", 1))
print(string.unpack("<j", string.pack("<j", math.maxinteger)))
print(string.unpack("<j", string.pack("<j", math.mininteger)))
print(string.unpack("<J", string.pack("<J", -1)))
