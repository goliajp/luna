-- a finalizer that raises inside an automatic step: the error reaches
-- the statement that allocated, through the message handler that runs
-- where the finalizer raised; a second object's finalizer runs later
local ran = {}
local function clean(s) return (string.gsub(tostring(s), "[^%s(]*:%d+: ", "")) end
local function mk(tag, bad)
  local p = newproxy(true)
  debug.getmetatable(p).__gc = function()
    ran[#ran + 1] = tag
    if bad then error("boom " .. tag) end
  end
  return p
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
print(ok, at, clean(e))
local h, m = xpcall(function()
  mk("c", true)
  for i = 1, 100000 do local _ = {i} end
end, function(m)
  local i = debug.getinfo(3, "Sl")
  return "handled in " .. i.what .. " line " .. i.currentline .. ", " .. tostring(m)
end)
print(h, clean(m))
collectgarbage()
print(table.concat(ran, " "))
-- the output follows where the automatic collection steps run
