-- v2.11 CORPUS-II: string.format hex float (%a) & alt (%#x).
print(string.format("%a", 1.0))       -- 0x1p+0
print(string.format("%A", 255))       -- 0X1.FEP+7
print(string.format("%#x", 255))      -- 0xff
print(string.format("%#X", 255))      -- 0XFF
print(string.format("%#o", 8))        -- 010
