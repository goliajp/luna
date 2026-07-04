-- v2.14 HD 5.4 seed: integer/float printing + math.ult, carried
-- behavior in the 5.4 dialect.
print(1, 1.0, 2^53)
print(math.type(1 // 1), math.type(1 / 1))
print(math.ult(1, -1), math.ult(-1, 1))
print(string.format("%d", 7), string.format("%g", 0.5))
