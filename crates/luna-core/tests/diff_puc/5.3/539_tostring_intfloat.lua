-- v2.14 HD 5.3 seed: tostring/tonumber across the new subtype.
print(tostring(5), tostring(5.0))
print(tonumber("5"), tonumber("5.0"))
print(math.type(tonumber("5")), math.type(tonumber("5.0")))
print(5 .. "", 5.0 .. "")
