-- while a coroutine runs, the collector marks the main thread's stack up
-- to its top only: a dead string left in a register above the call that
-- resumed the coroutine is freed
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return collectgarbage("count") * 1024
end
local before = count()
local s = "a" .. "b" .. "c" .. ("x"):rep(2000) .. "d"
coroutine.wrap(function()
  print("in the coroutine", string.format("%.0f", count() - before))
end)()
print("after", string.format("%.0f", count() - before), #s)
