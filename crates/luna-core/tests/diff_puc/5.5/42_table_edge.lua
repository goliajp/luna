-- v2.10 CORPUS: table edge cases (# semantics on holes).
local t = {1, 2, 3}
print(#t)   -- 3
t[10] = 100
-- # of table with hole is implementation-defined per Lua ref
-- so don't test # after hole. Test explicit key access.
print(t[10])
t[5] = 50
print(t[5], t[10])

-- rawlen bypasses __len
local sized = setmetatable({1,2,3,4,5}, {__len = function() return 100 end})
print(#sized, rawlen(sized))
