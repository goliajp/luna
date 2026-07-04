-- v2.14 CV.3: __eq fires only for same-type pairs, both tables.
local mt = { __eq = function() return true end }
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
local c = setmetatable({}, { __eq = function() return false end })
print(a == b, a == c, c == a)
print(a == 5, a ~= b)
print(rawequal(a, b), rawequal(a, a))
