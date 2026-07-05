-- v2.15 P2.5 (5.3): math.maxinteger / mininteger.
print(math.maxinteger)     -- 9223372036854775807
print(math.mininteger)     -- -9223372036854775808
print(math.maxinteger + 1 == math.mininteger)   -- true (wrap)
print(math.type(math.maxinteger))
