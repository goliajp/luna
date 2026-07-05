-- v2.15 P2.5: bitwise ops on integer-valued floats work in 5.4.
print(3.0 & 5.0)       -- 1 (both convert to int since they're exact)
print(4.0 | 2.0)        -- 6
print(6.0 ~ 3.0)        -- 5
-- but non-integer float errors
local ok = pcall(function() return 3.5 & 1 end)
print(ok)               -- false
