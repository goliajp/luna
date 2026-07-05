-- v2.15 P2.5: __close fires before value returns.
local closed = false
local function f()
  local x <close> = setmetatable({}, {__close = function() closed = true end})
  return "result"
end
local r = f()
print(closed, r)     -- true, result
