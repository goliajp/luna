-- v2.14 HD 5.3 seed: integer arithmetic wraps.
print(math.maxinteger + 1 == math.mininteger)
print(math.mininteger - 1 == math.maxinteger)
print(math.tointeger(3.0), math.tointeger(3.5))
print(math.type(2^62), math.type(1 << 62))
