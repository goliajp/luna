-- v2.15 P2.4 utf8: offset with positive index.
local s = "abc"
print(utf8.offset(s, 1))    -- 1
print(utf8.offset(s, 2))    -- 2
print(utf8.offset(s, 3))    -- 3
print(utf8.offset(s, 4))    -- 4 (past end)
