-- v2.11 CORPUS-II: __eq only fires for same-type operands.
local A = {}; A.__eq = function() return true end
local B = {}; B.__eq = function() return true end
local a1 = setmetatable({}, A)
local a2 = setmetatable({}, A)
local b1 = setmetatable({}, B)
print(a1 == a2)    -- true (both A)
-- Lua 5.4+: __eq fires across different mt if both have __eq
print(a1 == b1)
print(a1 == 42)    -- false (type mismatch, __eq not called)
print(a1 == nil)   -- false
