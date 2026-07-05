-- v2.15 P2.4 utf8: offset with third arg (start position).
local s = "hello"
print(utf8.offset(s, 1, 3))    -- 3 (byte 3 is start of char 1 from pos 3)
print(utf8.offset(s, 2, 3))    -- 4 (2nd char after byte 3)
print(utf8.offset(s, 0, 3))    -- 3 (0th char = current char at pos 3)
