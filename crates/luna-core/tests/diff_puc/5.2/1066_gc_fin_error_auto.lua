-- a finalizer that raises inside an automatic step: the error reaches
-- the statement that allocated, and a second
-- object's finalizer still runs later
local ran = {}
local function clean(s) return (string.gsub(tostring(s), "[^%s(]*:%d+: ", "")) end
local function mk(tag, bad)
  return setmetatable({}, {__gc = function()
    ran[#ran + 1] = tag
    if bad then error("boom " .. tag) end
  end})
end
do
  mk("a", true)
  mk("b", false)
end
local at = 0
local ok, e = pcall(function()
  for i = 1, 100000 do
    at = i
    local _ = {i}
  end
end)
print(ok, at > 0, clean(e))
local h, m = xpcall(function()
  mk("c", true)
  for i = 1, 100000 do local _ = {i} end
end, function(m) return "handled: " .. tostring(m) end)
print(h, clean(m))
collectgarbage()
print(table.concat(ran, " "))
-- the output follows where the automatic collection steps run
