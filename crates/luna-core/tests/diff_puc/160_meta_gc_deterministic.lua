-- v2.13 CORPUS-IV: __gc fires on collect; order for a single
-- unreachable object is deterministic under full collect.
local fired = 0
do
  local o = setmetatable({}, { __gc = function() fired = fired + 1 end })
  o = nil
end
collectgarbage("collect")
collectgarbage("collect")
print(fired)
-- __gc must be present at setmetatable time to mark finalizable
local late = {}
local mt = {}
setmetatable(late, mt)
mt.__gc = function() fired = fired + 100 end   -- added late: not marked
late = nil
collectgarbage("collect")
collectgarbage("collect")
print(fired)
