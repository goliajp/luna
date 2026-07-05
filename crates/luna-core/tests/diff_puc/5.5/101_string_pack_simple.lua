-- v2.12 CORPUS-III: string.pack/unpack basic (5.3+).
-- byte + short + int32 in native little-endian.
local s = string.pack("<Bi2i4", 0x41, 258, 100)
print(#s, string.byte(s, 1))    -- 7 65
local b, sh, i, pos = string.unpack("<Bi2i4", s)
print(b, sh, i, pos)
