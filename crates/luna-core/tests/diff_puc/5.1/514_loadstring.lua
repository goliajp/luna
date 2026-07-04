-- v2.14 HD 5.1 seed: loadstring (deprecated 5.2+).
print(loadstring("return 42")())
print(loadstring("return 1 + 2")())
local f, err = loadstring("syntax error here")
print(f == nil, err ~= nil)
