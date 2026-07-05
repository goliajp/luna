-- v2.15 P2.5 (5.3): shift by ≥64 or negative.
print(1 << 63)
print(1 << 64)             -- 0 (over-shift)
print(0xff >> 4)
print(0xff >> 8)            -- 0
print(1 << -1)              -- shift-by-negative → other direction
