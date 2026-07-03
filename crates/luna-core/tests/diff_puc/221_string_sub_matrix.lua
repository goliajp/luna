-- v2.13 CORPUS-IV: string.sub index matrix (negative, clamp,
-- inverted, zero).
local s = "abcdef"
print(s:sub(2), s:sub(2, 4), s:sub(-3))
print(s:sub(-3, -2), s:sub(1, -1), s:sub(0))
print(s:sub(4, 2) == "", s:sub(10) == "", s:sub(-10, 2))
print(s:sub(0, 0) == "", s:sub(-100, 100))
