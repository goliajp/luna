-- v2.11 CORPUS-II: local multi-decl adjust patterns.
local function three() return 1, 2, 3 end

local a, b, c = three()
print(a, b, c)

-- middle-position adjust to 1
local d, e, f = three(), 100
print(d, e, f)  -- 1 100 nil

-- at-end spreads
local g, h, i = 99, three()
print(g, h, i)  -- 99 1 2  (3rd return truncated)

local j, k, l, m = 99, three()
print(j, k, l, m)  -- 99 1 2 3
