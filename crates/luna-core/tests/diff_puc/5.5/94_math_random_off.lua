-- v2.11 CORPUS-II: math.randomseed determinism.
-- Both PUC and luna after randomseed(seed) should produce same sequence.
math.randomseed(42, 0)
local a = math.random(1, 1000)
local b = math.random(1, 1000)
math.randomseed(42, 0)
local c = math.random(1, 1000)
local d = math.random(1, 1000)
print(a == c, b == d)  -- true true
-- but the actual values may differ luna vs PUC, so don't print.
