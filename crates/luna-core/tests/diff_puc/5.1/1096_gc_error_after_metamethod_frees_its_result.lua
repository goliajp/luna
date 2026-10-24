-- 5.1's luaD_call checks for a collection when a metamethod returns, its
-- result on the stack: an error a finalizer raises in that check leaves
-- nothing holding the result. With a pause of 0 and a large step
-- multiplier each check runs a whole cycle: the check in the
-- metamethod's table constructor runs the first finalizer, which drops a
-- second proxy, and the check when the metamethod returns runs that one's
collectgarbage("setpause", 0)
collectgarbage("setstepmul", 1000000)
collectgarbage()
local weak = setmetatable({}, {__mode = "k"})
local function arm()
  local q = newproxy(true)
  getmetatable(q).__gc = function() error("gc") end
end
local obj = setmetatable({}, {__index = function()
  local t = {}
  weak[t] = true
  return t
end})
local function get() return obj.x end
local p = newproxy(true)
getmetatable(p).__gc = arm
p = nil
print(pcall(get))
collectgarbage("setpause", 200)
collectgarbage("setstepmul", 200)
for _ = 1, 3 do pcall(collectgarbage) end
local n = 0
for _ in pairs(weak) do n = n + 1 end
print(n)
