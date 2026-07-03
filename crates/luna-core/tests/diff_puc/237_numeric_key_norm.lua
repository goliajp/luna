-- v2.13 CORPUS-IV: float keys with integer values normalize to
-- integer keys; -0.0 keys to 0; NaN key errors.
local t = {}
t[1] = "one"
print(t[1.0])
t[2.0] = "two"
print(t[2], rawget(t, 2))
t[-0.0] = "zero"
print(t[0])
print((pcall(function() t[0 / 0] = "nan" end)))
print(t[0.5], (function() t[0.5] = "half" return t[0.5] end)())
