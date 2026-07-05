-- v2.15 P2.4 utf8: len with only start position.
local s = "abcdef"
print(utf8.len(s, 3))       -- 4 (c d e f = 4 codepoints from pos 3)
print(utf8.len(s, -2))      -- 2 (last 2 codepoints)
