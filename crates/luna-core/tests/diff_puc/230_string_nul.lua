-- v2.13 CORPUS-IV: embedded NUL bytes — length, sub, find, eq.
local s = "a\0b\0c"
print(#s, s:len())
print(s:sub(2, 2) == "\0")
print(s:find("\0", 1, true))
print(s == "a\0b\0c", s == "a\0b\0d")
print(("x\0y"):upper())
print(s:byte(1, 5))
