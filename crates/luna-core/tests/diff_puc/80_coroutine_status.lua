-- v2.11 CORPUS-II: coroutine.status transitions.
local co = coroutine.create(function()
  coroutine.yield()
end)
print(coroutine.status(co))     -- suspended
coroutine.resume(co)
print(coroutine.status(co))     -- suspended (after first yield)
coroutine.resume(co)
print(coroutine.status(co))     -- dead
