-- v2.14 HD 5.4 seed: <const> attribute arrives.
local k <const> = 10
print(k, k * 2)
local f, err = load("local c <const> = 1; c = 2")
print(f == nil, err ~= nil and err:find("const") ~= nil)
