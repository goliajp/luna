-- v2.13 CORPUS-IV: .. is right-associative; number coercion in
-- concat chains; precision of integer vs float in concat.
print(1 .. 2)
print(1 .. 2 .. 3)
print(1.5 .. "x")
print(1.0 .. "")
print(10 // 3 .. "|" .. 10 / 5)
local n = 0
local mt = {
  __concat = function(a, b)
    n = n + 1
    return "[" .. n .. "]"
  end,
}
local o = setmetatable({}, mt)
local r = "a" .. "b" .. o .. "c" .. "d"
print(r, n)
