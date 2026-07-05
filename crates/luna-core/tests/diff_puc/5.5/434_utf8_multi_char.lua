-- v2.15 P2.4 utf8: char with many code points at once.
local s = utf8.char(72, 101, 108, 108, 111)
print(s)
print(#s)
print(utf8.len(s))
