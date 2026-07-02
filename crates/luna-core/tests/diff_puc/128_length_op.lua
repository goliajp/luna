-- v2.12 CORPUS-III: # operator variants.
print(#"")
print(#"hello")
print(#{1, 2, 3, 4})
print(#{})
-- __len metamethod
local m = setmetatable({}, {__len = function() return 99 end})
print(#m)
-- rawlen bypasses
print(rawlen(m))    -- 0 (empty table)
print(rawlen({1,2,3,4}))
