-- v2.15 P2.5: coroutine.close closes a suspended coroutine.
local co = coroutine.create(function()
  coroutine.yield(1)
  coroutine.yield(2)
end)
print(coroutine.resume(co))
print(coroutine.status(co))         -- suspended
print(coroutine.close(co))
print(coroutine.status(co))         -- dead
