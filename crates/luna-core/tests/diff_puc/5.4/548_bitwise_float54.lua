-- v2.14 HD 5.4 seed: bitwise ops accept integral floats, reject
-- fractional ones.
print(3.0 & 1, 2.0 | 1)
print((pcall(function() return 3.5 & 1 end)))
print(~0, 1 << 62 ~= 0)
