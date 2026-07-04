-- v2.14 CV.3: close on suspended/dead/fresh coroutines.
local co = coroutine.create(function() coroutine.yield() end)
print(coroutine.close(co), coroutine.status(co))
local co2 = coroutine.create(function() end)
coroutine.resume(co2)
print(coroutine.close(co2))
local co3 = coroutine.create(function() coroutine.yield() end)
coroutine.resume(co3)
print(coroutine.close(co3), coroutine.status(co3))
