-- v2.14 CV.3: math edge results — abs/minmax/huge/fmod signs.
print(math.abs(-5), math.abs(5.5), math.abs(math.mininteger) == math.mininteger)
print(math.max(1, 2.5, -3), math.min(1, 2.5, -3))
print(math.huge > 0, -math.huge < 0, math.huge == math.huge + 1)
print(math.fmod(5, 3), math.fmod(-5, 3), math.fmod(5, -3))
print(math.floor(-2.5), math.ceil(-2.5), math.floor(2^60))
