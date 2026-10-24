-- 5.1's luaD_call checks for a collection when the call returns, inside
-- the pcall that made it: an error a finalizer raises there is caught by
-- that pcall (or xpcall, under its handler). With a pause of 0 and a large
-- step multiplier each check runs a whole cycle, so the first check after
-- the proxy is dropped, the one when f returns, runs its finalizer; the
-- error leaves the threshold the finalizer ran under, and a full
-- collection sets it to 0 again
collectgarbage("setpause", 0)
collectgarbage("setstepmul", 1000000)
collectgarbage()
local function f() return 1 end
local function h(m) return "handled " .. m end
local p = newproxy(true)
getmetatable(p).__gc = function() error("gc") end
p = nil
print(pcall(f))
collectgarbage()
p = newproxy(true)
getmetatable(p).__gc = function() error("gc again") end
p = nil
print(xpcall(f, h))
collectgarbage("setpause", 200)
collectgarbage("setstepmul", 200)
print("after")
