-- v2.13 CORPUS-IV: tostring/tonumber coercion matrix.
print(tostring(nil), tostring(true), tostring(false))
print(tostring(42), tostring(42.0), tostring(-0.5))
print(tonumber("42"), tonumber("42.5"), tonumber("  10  "))
print(tonumber("0x1F"), tonumber("1e3"), tonumber(".5"))
print(tonumber("abc"), tonumber(""), tonumber("10a"))
print(tonumber("42", 10), tonumber(true))
print(math.type(tonumber("42")), math.type(tonumber("42.0")))
