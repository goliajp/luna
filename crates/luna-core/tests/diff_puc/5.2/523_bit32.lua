-- v2.14 HD 5.2 seed: bit32 library (5.2 only; removed in 5.3).
print(bit32.band(0xF0, 0x3C), bit32.bor(0xF0, 0x0F))
print(bit32.bxor(0xFF, 0x0F), bit32.bnot(0))
print(bit32.lshift(1, 4), bit32.rshift(256, 4))
print(bit32.arshift(-16, 2))
print(bit32.extract(0xABCD, 4, 8))
