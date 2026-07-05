-- v2.12 CORPUS-III: math.tointeger edges + integer wrap on
-- plain arithmetic (5.3+ wraps, no error).
print(math.tointeger(3.0), math.tointeger(3.5), math.tointeger(2^62))
print(math.type(math.tointeger(3.0)))
print(math.maxinteger + 1 == math.mininteger)
print(math.mininteger - 1 == math.maxinteger)
print(math.type(math.maxinteger), math.type(math.huge))
print(math.tointeger(math.huge))
