-- v2.15 P2.5 (5.2): math.randomseed determinism.
math.randomseed(42)
local a = math.random()
math.randomseed(42)
local b = math.random()
print(a == b)
