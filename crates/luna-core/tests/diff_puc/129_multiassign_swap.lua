-- v2.12 CORPUS-III: multi-assign parallel semantics.
local a, b, c = 1, 2, 3
a, b, c = c, a, b
print(a, b, c)   -- 3 1 2

-- table field swap
local t = {[1] = "a", [2] = "b", [3] = "c"}
t[1], t[3] = t[3], t[1]
print(t[1], t[2], t[3])
