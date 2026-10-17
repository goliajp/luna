-- a 5.1 finalizer's error leaves through the finalizer itself: the
-- message handler of the call that collected runs where it raised
-- (no automatic step: the explicit collection runs the finalizer)
collectgarbage("stop")
local function clean(s) return (string.gsub(tostring(s), "[^%s(]*:%d+: ", "")) end
local p = newproxy(true)
debug.getmetatable(p).__gc = function()
  error("boom")
end
p = nil
local ok, m = xpcall(function() collectgarbage() end, function(m)
  local i = debug.getinfo(3, "Sl")
  return "handled in " .. i.what .. " line " .. i.currentline .. ", " .. clean(m)
end)
print(ok, m)
-- the collector runs again afterwards
local q = newproxy(true)
debug.getmetatable(q).__gc = function() print("second") end
q = nil
collectgarbage()
print("end")
