-- v2.14 CV.3: unpack returns the next-read offset; explicit init.
local blob = string.pack("<i2i2i2", 10, 20, 30)
local a, pos = string.unpack("<i2", blob)
print(a, pos)
local b, pos2 = string.unpack("<i2", blob, pos)
print(b, pos2)
local c, d, pos3 = string.unpack("<i2i2", blob, 3)
print(c, d, pos3)
