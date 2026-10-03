-- coroutine.isyieldable of a dead coroutine: PUC asks only whether the
-- thread sits in a non-yieldable call
local co = coroutine.create(function() end)
print(coroutine.isyieldable(co))
coroutine.resume(co)
print(coroutine.status(co), coroutine.isyieldable(co))
local bad = coroutine.create(function() error('x') end)
coroutine.resume(bad)
print(coroutine.status(bad), coroutine.isyieldable(bad))
