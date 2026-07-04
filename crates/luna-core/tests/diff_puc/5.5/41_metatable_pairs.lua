-- v2.10 CORPUS: __pairs metamethod (Lua 5.2+ deprecated in 5.4, still works).
-- Use next() explicitly to avoid __pairs vs raw pairs divergence.
local t = {a=1, b=2, c=3}
local keys = {}
for k in pairs(t) do keys[#keys+1] = k end
table.sort(keys)
print(table.concat(keys, "|"))

-- Test that rawget bypasses metatable.
local mt = {__index = function() return "META" end}
local obj = setmetatable({}, mt)
print(obj.anything)   -- META
print(rawget(obj, "anything"))  -- nil
