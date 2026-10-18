-- the main thread seen from a coroutine it resumed: its levels, and what
-- coroutine.resume and a wrapped function keep on its stack meanwhile
local main = coroutine.running()
local function tb(th, msg)
  local t = debug.traceback(th, msg)
  print((t:gsub("\n%s*[^:\n]*:%d+:", "\n\t<pos>:")))
end
local co = coroutine.wrap(function(...)
  tb(main, "tb")
  for l = 0, 3 do
    local i = debug.getinfo(main, l, "Sn")
    print(l, i and i.what, i and i.name)
  end
  print(select("#", debug.getlocal(main, 0, 1)), (debug.getlocal(main, 0, 1)))
  print((debug.getlocal(main, 1, 1)))
end)
co("a", "b")
local co2 = coroutine.create(function()
  tb(main, "tb2")
  local name, v = debug.getlocal(main, 0, 1)
  print(name, type(v))
  print((debug.getlocal(main, 0, 2)))
end)
coroutine.resume(co2, "x")
