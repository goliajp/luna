-- v3.1 debug slice: getinfo 'r' reports transferred values only for the
-- function a call/return hook interrupted; line hooks transfer none.
local log = {}
debug.sethook(function(ev)
  local r = debug.getinfo(2, "r")
  local here = debug.getinfo(1, "r")
  log[#log + 1] = ev .. " " .. r.ftransfer .. "/" .. r.ntransfer .. " " .. here.ftransfer .. "/" .. here.ntransfer
end, "cl")
local function f(a, b) return a end
f(1, 2)
debug.sethook()
for _, l in ipairs(log) do print(l) end
