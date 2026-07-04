-- v2.14 HD 5.2 seed: xpcall forwards extra args (new in 5.2).
local ok, v = xpcall(function(a, b) return a + b end, print, 3, 4)
print(ok, v)
