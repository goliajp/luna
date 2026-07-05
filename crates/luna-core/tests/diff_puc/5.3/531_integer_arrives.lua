-- v2.14 HD 5.3 seed: the integer subtype arrives — 1 vs 1.0
-- print differently; math.type distinguishes.
print(1, 1.0, 3 / 1, 3 // 1)
print(math.type(1), math.type(1.0), math.type("x"))
print(1 == 1.0, math.maxinteger, math.mininteger)
