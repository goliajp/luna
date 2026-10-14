local function mk(f)
  if newproxy then local o = newproxy(true); getmetatable(o).__gc = f; return o end
  return setmetatable({}, {__gc = f})
end
do local x = mk(function() error("boom") end) end
print(xpcall(collectgarbage, function(m) return "handled: " .. tostring(m) end))
