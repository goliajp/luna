-- v3.1 debug slice: 5.1/5.2 name a hook function after the instruction the
-- hooked function is at — for a call hook, its first one.
local names = {}
debug.sethook(function(ev)
  local i = debug.getinfo(1, "n")
  names[#names + 1] = ev .. ":" .. tostring(i.namewhat) .. ":" .. tostring(i.name)
end, "c")
local function f(a, b) local c = a + b return c end
f(1, 2)
debug.sethook()
print(names[1])
