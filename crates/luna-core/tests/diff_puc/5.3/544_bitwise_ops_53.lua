-- v2.15 P2.5 (5.3): bitwise operators &, |, ~, <<, >>.
print(0xf0 & 0x0f)
print(0xf0 | 0x0f)
print(0xff ~ 0x55)
print(~0 & 0xff)
print(1 << 4)
print(256 >> 2)
-- integer type preserved
print(math.type(0xff & 0x0f))
