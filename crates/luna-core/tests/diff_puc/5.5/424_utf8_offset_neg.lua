-- v2.15 P2.4 utf8: offset with negative index.
local s = "abcdef"
print(utf8.offset(s, -1))    -- 6 (last char)
print(utf8.offset(s, -2))    -- 5
print(utf8.offset(s, -3))    -- 4
