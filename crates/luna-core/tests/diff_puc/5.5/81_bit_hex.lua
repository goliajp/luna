-- v2.11 CORPUS-II: bitwise + hex literals.
print(0xff | 0xff00)
print(0xff00 >> 8)
print(0xdeadbeef & 0xffff)
print(0xdeadbeef & 0xffff0000)
-- Two's complement all-ones
print(~0)                     -- -1
print(~0 & 0xff)              -- 255
print(0x7fffffffffffffff)     -- max i64
