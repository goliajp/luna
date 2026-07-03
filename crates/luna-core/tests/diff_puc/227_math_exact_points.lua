-- v2.13 CORPUS-IV: math functions at exactly-representable
-- points (no libm rounding divergence risk).
print(math.sqrt(4), math.sqrt(0), math.sqrt(2.25))
print(math.exp(0), math.log(1))
print(math.log(8, 2), math.log(100, 10))
print(math.sin(0), math.cos(0), math.tan(0))
print(math.sqrt(-1) ~= math.sqrt(-1))
print(math.type(math.sqrt(4)))
