-- v2.14 CV.3: resuming a dead coroutine — (false, msg).
local co = coroutine.create(function() end)
coroutine.resume(co)
print(coroutine.resume(co))
print(coroutine.resume(co, 1, 2))
