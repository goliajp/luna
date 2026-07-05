-- v2.14 HD 5.4 seed: coroutine.close arrives.
local co = coroutine.create(function() coroutine.yield() end)
coroutine.resume(co)
print(coroutine.status(co))
print(coroutine.close(co))
print(coroutine.status(co))
print(type(coroutine.close))
