-- v2.15 P2.4 utf8: char(0) is a valid single-byte string.
local s = utf8.char(0)
print(#s)            -- 1
print(string.byte(s, 1))   -- 0
-- codepoint reads it back
print(utf8.codepoint(s, 1))
