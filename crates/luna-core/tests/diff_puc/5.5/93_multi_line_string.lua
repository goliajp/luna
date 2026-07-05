-- v2.11 CORPUS-II: long-bracket string literals.
local s = [[first
second
third]]
print(#s)
print(s == "first\nsecond\nthird")

local nested = [==[hi [[embedded]] bye]==]
print(nested)
