-- v2.15 P2.4 utf8: len with i and j range.
local s = "abcdef"
print(utf8.len(s, 1, -1))    -- 6
print(utf8.len(s, 2, 4))     -- 3 (b c d)
print(utf8.len(s, 3, 3))     -- 1 (c)
print(utf8.len(s, 1, 0))     -- 0 (empty range)
