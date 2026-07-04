-- v2.10 CORPUS: multi-assignment semantics.
local a, b = 1, 2
a, b = b, a
print(a, b)  -- 2 1

-- rhs evaluated before lhs write
local x = 10
local y = 20
x, y = y + 1, x + 1
print(x, y)  -- 21 11 (both rhs from OLD x, y)

-- table field swap
local t = {1, 2}
t[1], t[2] = t[2], t[1]
print(t[1], t[2])
