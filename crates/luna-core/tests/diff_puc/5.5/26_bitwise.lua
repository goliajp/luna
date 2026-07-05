-- v2.10 CORPUS: bitwise ops (Lua 5.3+).
print(0xff & 0x0f)     -- 15
print(0x0f | 0xf0)     -- 255
print(0xff ~ 0x0f)     -- 240
print(~0 & 0xff)       -- 255
print(1 << 8)          -- 256
print(0x100 >> 4)      -- 16
print(0xffffffff & 0xff)
