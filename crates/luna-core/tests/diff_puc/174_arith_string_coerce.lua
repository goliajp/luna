-- v2.13 CORPUS-IV: arithmetic string coercion — numeric strings
-- convert; metamethod fires only when coercion fails.
print("10" + 5, "3" * "4", "2.5" - 1)
print(math.type("10" + 0))
print(math.type("10.0" + 0))
print("0x10" + 0)
print(" 7 " + 1)   -- surrounding spaces OK
local o = setmetatable({}, { __add = function() return "obj_add" end })
print("10" + o)    -- coercion of "10" ok, o forces metamethod
print((pcall(function() return "abc" + 1 end)))
