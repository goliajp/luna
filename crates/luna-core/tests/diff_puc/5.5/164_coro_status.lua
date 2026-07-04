-- v2.13 CORPUS-IV: coroutine.status transitions +
-- isyieldable/running.
print(coroutine.isyieldable())
local co
co = coroutine.create(function()
  print("inside", coroutine.status(co), coroutine.isyieldable())
  local main, ismain = coroutine.running()
  print("running_is_co", main == co, ismain)
  coroutine.yield()
end)
print("before", coroutine.status(co))
coroutine.resume(co)
print("suspended", coroutine.status(co))
coroutine.resume(co)
print("after", coroutine.status(co))
local main, ismain = coroutine.running()
print("main_flag", ismain)
