-- v2.11 CORPUS-II: __newindex as function preserves rawset.
local logged = {}
local obj = setmetatable({}, {__newindex = function(t, k, v)
  logged[#logged+1] = k .. "=" .. tostring(v)
  rawset(t, k, v)
end})
obj.x = 10
obj.y = 20
obj.z = 30
print(logged[1], logged[2], logged[3])
print(obj.x, obj.y, obj.z)
-- overwrite existing: __newindex NOT called for existing keys
obj.x = 100
print(#logged)   -- still 3
print(obj.x)
