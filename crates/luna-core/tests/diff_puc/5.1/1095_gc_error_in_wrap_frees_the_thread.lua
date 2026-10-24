-- coroutine.wrap pushes the new thread and checks for a collection
-- before lua_pushcclosure makes the wrapper: an error a finalizer raises
-- in that check leaves nothing holding the thread or its function. With a
-- pause of 0 and a large step multiplier each check runs a whole cycle:
-- the check in lua_newthread runs the first finalizer, which drops a
-- second proxy, and the check before lua_pushcclosure runs that one's
collectgarbage("setpause", 0)
collectgarbage("setstepmul", 1000000)
collectgarbage()
local weak = setmetatable({}, {__mode = "k"})
local function arm()
  local q = newproxy(true)
  getmetatable(q).__gc = function() error("gc") end
end
local f = function() end
weak[f] = true
local p = newproxy(true)
getmetatable(p).__gc = arm
p = nil
print(pcall(coroutine.wrap, f))
f = nil
collectgarbage("setpause", 200)
collectgarbage("setstepmul", 200)
for _ = 1, 3 do pcall(collectgarbage) end
local n = 0
for _ in pairs(weak) do n = n + 1 end
print(n)
