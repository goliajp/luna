-- v2.14 CV.3: sub index normalization matrix.
local s = "abcdef"
print(s:sub(2, 4), s:sub(-3), s:sub(-3, -2))
print(s:sub(0), s:sub(1, 100), s:sub(4, 2) == "")
print(s:sub(-100, 2), s:sub(7) == "")
