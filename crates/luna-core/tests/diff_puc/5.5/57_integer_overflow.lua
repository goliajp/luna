-- v2.11 CORPUS-II: integer overflow (wraps in Lua 5.3+ int).
print(math.maxinteger + 1)
print(math.mininteger - 1)
print(math.maxinteger * 2)
print(-math.mininteger)  -- overflows back to mininteger
-- unsigned display
print(math.maxinteger + math.maxinteger)
-- shift semantics
print(1 << 62)
print(1 << 63)
print(1 << 64)  -- 0 (out-of-range shift)
