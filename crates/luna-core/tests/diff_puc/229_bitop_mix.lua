-- v2.13 CORPUS-IV: bitwise mix + precedence + float-with-int-rep
-- coercion + no-int-rep error.
print(~0, ~~5)
print(0xF0 & 0x3C, 0xF0 | 0x0F, 0xF0 ~ 0xFF)
print(1 | 2 & 3)
print(3.0 & 1)
print((pcall(function() return 3.5 & 1 end)))
print(5 & -1, -2 | 0)
