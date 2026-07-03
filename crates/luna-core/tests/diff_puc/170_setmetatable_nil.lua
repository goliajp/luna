-- v2.13 CORPUS-IV: setmetatable(t, nil) clears; returns t itself.
local mt = { __index = function() return "x" end }
local t = setmetatable({}, mt)
print(t.missing)
local same = setmetatable(t, nil)
print(same == t, getmetatable(t), t.missing)
-- setmetatable rejects non-table/non-nil second arg
print((pcall(setmetatable, {}, 42)))
print((pcall(setmetatable, {}, "mt")))
