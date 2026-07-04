-- v2.11 CORPUS-II: math constant folding.
print(math.pi > 3 and math.pi < 4)
print(math.huge > 1e300)
print(math.huge == math.huge)
print(math.huge - 1 == math.huge)
-- nan self-comparison
local nan = 0/0
print(nan == nan)   -- false
print(nan ~= nan)   -- true
