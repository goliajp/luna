-- v2.13 CORPUS-IV: decimal integer literals beyond maxinteger
-- become floats (5.3+); hex literals wrap.
print(math.type(9223372036854775807))
print(math.type(9223372036854775808))
print(9223372036854775808 == 2^63)
print(math.type(0xFFFFFFFFFFFFFFFF), 0xFFFFFFFFFFFFFFFF)
print(math.type(123456789012345678901234567890))
