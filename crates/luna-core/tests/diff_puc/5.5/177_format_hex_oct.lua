-- v2.13 CORPUS-IV: string.format %x/%X/%o + width/precision.
print(string.format("%x", 255), string.format("%X", 255))
print(string.format("%#x", 255), string.format("%#o", 8))
print(string.format("%08x", 3735928559))
print(string.format("%o", 64))
print(string.format("%x", math.maxinteger))
print(string.format("%x", -1))
