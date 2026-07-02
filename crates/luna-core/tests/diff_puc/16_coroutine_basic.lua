-- v2.10 CORPUS: coroutine.yield + resume.
local co = coroutine.create(function(a, b)
  local c = coroutine.yield(a + b)
  local d = coroutine.yield(c * 2)
  return c, d
end)
print(coroutine.resume(co, 1, 2))    -- true 3
print(coroutine.resume(co, 10))      -- true 20
print(coroutine.resume(co, 99))      -- true 10 99
print(coroutine.status(co))          -- dead
