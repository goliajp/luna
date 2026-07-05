-- v2.15 P2.4 utf8: codepoint with range.
local s = "abcdef"
print(utf8.codepoint(s, 1, 3))    -- 97, 98, 99
print(utf8.codepoint(s, -3))      -- 100 (d)
print(utf8.codepoint(s, -3, -1))  -- 100, 101, 102
