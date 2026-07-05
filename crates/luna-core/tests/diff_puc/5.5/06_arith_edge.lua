-- v2.10 CORPUS: arithmetic edge cases (Lua 5.5 integer/float boundaries).
print(math.maxinteger)
print(math.mininteger)
print(math.maxinteger + 1 == math.mininteger)  -- integer wraparound
print(math.type(1))       -- integer
print(math.type(1.0))     -- float
print(math.type("1"))     -- fail (nil)
print(1 // 0.5)           -- floor div float
print(math.floor(3.7))
print(math.ceil(3.2))
print(math.abs(-42))
print(math.abs(0))
print(math.max(1, 5, 3, 4))
print(math.min(1, 5, 3, 4))
print(0 == -0)            -- integer 0 == 0
print(0.0 == -0.0)        -- float
print(1 == 1.0)           -- int == float coercion
print(1 // 1)             -- 1 integer
print(1.0 // 1)           -- 1.0 float
