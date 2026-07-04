-- v2.14 CV.3: status in every reachable state.
local co
co = coroutine.create(function()
  print("inside", coroutine.status(co))
  coroutine.yield()
end)
print("fresh", coroutine.status(co))
coroutine.resume(co)
print("suspended", coroutine.status(co))
coroutine.resume(co)
print("dead", coroutine.status(co))
