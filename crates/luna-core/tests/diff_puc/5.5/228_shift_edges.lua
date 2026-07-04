-- v2.13 CORPUS-IV: shifts — negative amounts reverse direction,
-- >= 64 yields 0, >> is logical (unsigned).
print(1 << 4, 16 >> 4)
print(1 << 64, 1 >> 64, 1 << 100)
print(1 << -4, 16 >> -4)
print(-1 >> 1)
print(-1 << 1)
print(0x80 >> 7, 1 << 63)
