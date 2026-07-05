-- v2.15 P2.5 (5.3): string.pack integer formats.
local s = string.pack("<i4", 258)
print(#s)               -- 4
local n = string.unpack("<i4", s)
print(n)                -- 258

-- big-endian
local sb = string.pack(">i4", 258)
print(string.byte(sb, 1), string.byte(sb, 4))  -- 0, 2
