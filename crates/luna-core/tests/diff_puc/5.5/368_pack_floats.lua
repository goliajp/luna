-- v2.14 CV.3: float/double round trips.
print(string.unpack("<f", string.pack("<f", 0.5)))
print(string.unpack("<d", string.pack("<d", 3.141592653589793)))
print(string.unpack(">d", string.pack(">d", -0.25)))
print(#string.pack("<f", 1), #string.pack("<d", 1))
print(string.unpack("<n", string.pack("<n", 2.5)))
