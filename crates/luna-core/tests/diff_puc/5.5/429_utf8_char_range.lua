-- v2.15 P2.4 utf8: char across byte-length boundaries.
-- 1-byte: 0x00-0x7F
-- 2-byte: 0x80-0x7FF
-- 3-byte: 0x800-0xFFFF
-- 4-byte: 0x10000-0x10FFFF
print(#utf8.char(0x7F))            -- 1
print(#utf8.char(0x80))            -- 2
print(#utf8.char(0x7FF))           -- 2
print(#utf8.char(0x800))           -- 3
print(#utf8.char(0xFFFF))          -- 3
print(#utf8.char(0x10000))         -- 4
print(#utf8.char(0x10FFFF))        -- 4
