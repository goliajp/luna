-- the buffer's slot under a `__tostring` that `string.format` calls: the
-- metamethod lookup before the call may collect, and the slot is pushed
local mt = {__tostring = function()
  local name, v = debug.getlocal(2, 3)
  print(type(v))
  return "s"
end}
print(string.format("%s", setmetatable({}, mt)))
print(string.format("%5s|", setmetatable({}, mt)))
