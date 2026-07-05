-- v2.15 P2.4 utf8: offset with multibyte string.
local s = "aé b"     -- a(1) é(2) space(1) b(1)
print(utf8.offset(s, 1))    -- 1 (a)
print(utf8.offset(s, 2))    -- 2 (é starts at byte 2)
print(utf8.offset(s, 3))    -- 4 (space, after é)
print(utf8.offset(s, 4))    -- 5 (b)
