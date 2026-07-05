-- v2.15 P2.4 utf8: offset returning nil for out-of-range.
local s = "abc"
print(utf8.offset(s, 5))    -- nil (5th char doesn't exist)
print(utf8.offset(s, -4))   -- nil (before start)
