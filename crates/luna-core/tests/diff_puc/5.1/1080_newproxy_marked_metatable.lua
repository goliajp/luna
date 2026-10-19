-- rule: lbaselib.luaB_newproxy.1 lapi.lua_setmetatable.1
-- A proxy that a step marks while newproxy allocates its metatable still
-- has that metatable, __gc and all, when the proxy is collected
local function obj(fin)
  local u = newproxy(true)
  getmetatable(u).__gc = fin
  return u
end
local function run(n, pause, mul)
  collectgarbage("setpause", pause)
  collectgarbage("setstepmul", mul)
  local ran = 0
  local keep = {}
  for i = 1, n do
    keep[i] = obj(function() ran = ran + 1 end)
    local pad = {i}
  end
  keep = nil
  collectgarbage()
  collectgarbage()
  print(n, pause, mul, ran)
end
for _, mul in ipairs({50, 100, 200, 400}) do
  run(500, 100, mul)
  run(5000, 100, mul)
end
