-- v2.15 P2.4 utf8: char basic.
print(utf8.char(65))            -- "A"
print(utf8.char(72, 105))       -- "Hi"
print(utf8.char(0x1F600))       -- 4-byte emoji
print(#utf8.char(0x1F600))       -- 4 bytes
print(utf8.char(0))              -- U+0000
print(#utf8.char(0))             -- 1 byte
