-- v2.14 CV.3: x padding and ! alignment.
print(#string.pack("bxi1", 1, 2))
local packed = string.pack("bxxB", 7, 9)
print(#packed, packed:byte(1), packed:byte(4))
print(string.unpack("bxxB", packed))
print(#string.pack("!4bi4", 1, 2))
print(#string.pack("!1bi4", 1, 2))
