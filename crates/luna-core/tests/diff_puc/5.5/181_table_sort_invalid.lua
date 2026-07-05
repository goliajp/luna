-- v2.13 CORPUS-IV: table.sort with an invalid (inconsistent)
-- comparator raises "invalid order function for sorting"; only
-- the ok flag is compared (wording location varies).
local t = {}
for i = 1, 32 do t[i] = i % 4 end
local ok = pcall(table.sort, t, function(a, b) return true end)
print(ok)
-- comparator errors propagate
local ok2, err = pcall(table.sort, { 3, 1, 2 }, function() error("cmp_boom", 0) end)
print(ok2, err)
