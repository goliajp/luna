-- v2.13 CORPUS-IV: math.floor/ceil/abs subtype behavior (5.3+:
-- integer-valued results are integers).
print(math.floor(3.7), math.ceil(3.2), math.floor(-3.7), math.ceil(-3.2))
print(math.type(math.floor(3.7)), math.type(math.floor(3)))
print(math.abs(-5), math.abs(5.5), math.abs(math.mininteger + 1))
print(math.type(math.abs(-5)), math.type(math.abs(-5.0)))
print(math.floor(2^60), math.type(math.floor(2^60)))
print(math.ult(1, 2), math.ult(-1, 1), math.ult(1, -1))
