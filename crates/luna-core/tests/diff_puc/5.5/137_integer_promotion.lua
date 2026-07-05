-- v2.12 CORPUS-III: integer/float subtype propagation (5.3+).
print(math.type(1 + 1), math.type(1 + 1.0), math.type(1.0 + 1.0))
print(math.type(2 ^ 2), math.type(7 / 2), math.type(7 // 2))
print(math.type(7.0 // 2), math.type(7 % 2), math.type(7.0 % 2))
print(1 == 1.0, math.type(-0.0), 0 == -0.0)
print(3 // 1, 3.0 // 1, 3 / 1)
