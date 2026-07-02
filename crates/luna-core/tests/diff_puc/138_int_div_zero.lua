-- v2.12 CORPUS-III: division by zero — integer ops error,
-- float ops yield inf/nan. Error wording is a known
-- cross-dialect choice, so only the ok flag is compared.
local ok1 = pcall(function() return 1 // 0 end)
local ok2 = pcall(function() return 1 % 0 end)
print(ok1, ok2)
print(1.0 // 0.0 == math.huge, -1.0 // 0.0 == -math.huge)
print(1 / 0 == math.huge, math.type(1 / 0))
local nan = 0.0 / 0.0
print(nan == nan, nan ~= nan)
