-- v2.10 CORPUS: local decl semantics.
local a, b, c = 1, 2, 3
print(a, b, c)
-- fewer vals than names → nil pad
local x, y, z = 1
print(x, y, z)  -- 1 nil nil
-- more vals than names → excess dropped
local p, q = 1, 2, 3, 4
print(p, q)

-- assignment from function returns
local function ret2() return 10, 20 end
local m, n, o = ret2(), 99
print(m, n, o)  -- 10 99 nil (ret2 adjusted to 1 in middle)

local k, l = ret2()
print(k, l)  -- 10 20 (last pos, spreads)
