-- v2.14 HD 5.3 seed: string.pack/unpack arrive.
local s = string.pack("<i4", 258)
print(#s, string.byte(s, 1), string.byte(s, 2))
print(string.unpack("<i4", s))
print(string.packsize("<i4i8"))
