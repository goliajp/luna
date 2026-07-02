-- v2.10 CORPUS: multi-return + adjust semantics.
local function pair() return 1, 2 end
local function trip() return 10, 20, 30 end
local a, b = pair()
print(a, b)
local x, y, z = pair()
print(x, y, z)   -- 1 2 nil

-- adjust in middle of arglist
print("head", pair(), "tail")  -- pair() adjusts to 1 return
print("head", trip())          -- trip() spreads
print(trip(), "tail")          -- but truncated in middle

-- in table constructor
local t = {pair(), trip()}
print(#t, t[1], t[2], t[3], t[4])
