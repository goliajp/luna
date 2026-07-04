-- v2.11 CORPUS-II: coroutine.close.
local co = coroutine.create(function()
  coroutine.yield(1)
  coroutine.yield(2)
end)
print(coroutine.resume(co))       -- true 1
print(coroutine.status(co))       -- suspended
print(coroutine.close(co))        -- true
print(coroutine.status(co))       -- dead
