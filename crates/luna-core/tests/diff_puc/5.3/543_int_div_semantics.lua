-- v2.15 P2.5 (5.3): // integer division rounds toward -inf.
print(7 // 2)         -- 3
print(-7 // 2)        -- -4 (floor, not truncate)
print(7 // -2)        -- -4
print(-7 // -2)       -- 3
print(0 // 1)          -- 0
print(math.type(7 // 2))  -- integer
