-- v2.12 CORPUS-III: string.format with hex/octal/int flavor.
print(string.format("%x", 0))
print(string.format("%X", 0xdeadbeef))
print(string.format("%o", 0))
print(string.format("%o", 63))
print(string.format("%08x", 0xff))
