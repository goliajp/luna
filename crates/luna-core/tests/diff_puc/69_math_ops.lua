-- v2.11 CORPUS-II: math library.
-- NOTE: math.modf luna returns float where PUC returns integer;
-- excluded pending semantic alignment (v3.0 stretch).
print(math.floor(3.7))
print(math.floor(-3.2))
print(math.ceil(3.2))
print(math.ceil(-3.7))
print(math.fmod(10, 3))
print(math.exp(0))
print(string.format("%.6f", math.log(math.exp(2))))
print(string.format("%.6f", math.log(100, 10)))
