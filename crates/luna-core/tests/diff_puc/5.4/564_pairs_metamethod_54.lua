-- v2.15 P2.5: __pairs metamethod (5.4 supports; deprecated).
local t = setmetatable({}, {__pairs = function(self)
  local i = 0
  return function()
    i = i + 1
    if i <= 3 then return i, i * 10 end
  end
end})
for k, v in pairs(t) do print(k, v) end
